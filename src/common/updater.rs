//! Self-update: fetch a small JSON manifest, compare versions, download the
//! installer, verify its SHA-256, and start it.
//!
//! The manifest format is unchanged from the Python builds, so copies of the
//! old version already in the field can update to this one:
//!
//! ```json
//! { "version": "3.0.1",
//!   "url": "https://github.com/<owner>/<repo>/releases/download/v3.0.1/<App>-3.0.1-setup.exe",
//!   "sha256": "<64 hex digits>",
//!   "notes": "Short release notes." }
//! ```
//!
//! Hardening compared with the Python updater:
//! * The installer URL must be HTTPS, inside this project's own release
//!   downloads (`AppInfo::download_prefix`), under the tag `v<version>`;
//!   `file://`, `http://` and other hosts are refused. Redirects must stay on
//!   HTTPS.
//! * The version string is validated before it is used in a file name (the
//!   old code would have accepted path separators from the manifest).
//! * Downloads go to a newly created, randomly named folder inside the user's
//!   own profile (`%LOCALAPPDATA%`), never to a predictable path in a %TEMP%
//!   that IT may have pointed at a shared drive, and the file is created new.
//! * After download the file is reopened with sharing that forbids writing,
//!   renaming or deleting it, and hashed again from disk. That lock is held
//!   until the installer has finished (or has closed this program), which
//!   covers the moment Inno Setup re-runs the same file elevated.
//! * Manifest and installer sizes are capped; every network operation has a
//!   time limit.
//! * The installer is started without elevation. For a machine-wide install,
//!   Inno Setup asks for administrator approval itself, and can then relaunch
//!   the program as the original (non-admin) user.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, SystemTime};

use super::http::{self, Client, HttpError};
use super::policy::{InstallScope, Policy};
use super::{AppInfo, fsutil, json, paths};

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_INSTALLER: u64 = 256 * 1024 * 1024;
const MAX_NOTES_CHARS: usize = 2000;

/// Per network operation (connect, each read): short, as in the Python
/// version, so a laptop off the VPN is not kept waiting.
const CHECK_STEP_TIMEOUT: Duration = Duration::from_secs(6);
/// Whole check, including the redirect to GitHub's download host.
const CHECK_TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Per read while downloading; the Python version allowed 300 s per read.
const DOWNLOAD_STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// Whole download; generous for slow links.
const DOWNLOAD_TOTAL_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Leftover download folders older than this are removed.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
pub enum UpdateError {
    /// An administrator turned updates off in policy.ini.
    DisabledByPolicy,
    /// policy.ini exists but could not be read; updates stay off to be safe.
    PolicyUnreadable(String),
    Network(HttpError),
    /// The manifest was fetched but is not acceptable.
    BadManifest(&'static str),
    /// The installer did not match the manifest's SHA-256.
    ChecksumMismatch,
    Cancelled,
    Io(io::Error),
    /// Installing updates is only possible on Windows.
    Unsupported,
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::DisabledByPolicy => f.write_str("Updates are managed by your administrator."),
            UpdateError::PolicyUnreadable(why) => {
                write!(f, "Updates are turned off because policy.ini could not be read ({why}).")
            }
            UpdateError::Network(e @ HttpError::Tls(_)) => write!(
                f,
                "Could not reach the update server: {e}.\n\nIf your network inspects secure connections, \
                 IT can add its root certificate to policy.ini (ExtraRootCertificates)."
            ),
            UpdateError::Network(e) => write!(f, "Could not reach the update server: {e}."),
            UpdateError::BadManifest(why) => write!(f, "The update information is not valid ({why})."),
            UpdateError::ChecksumMismatch => {
                f.write_str("The download was corrupt or tampered with (checksum mismatch), so it was not run.")
            }
            UpdateError::Cancelled => f.write_str("The download was cancelled."),
            UpdateError::Io(e) => write!(f, "Could not save or start the update: {e}."),
            UpdateError::Unsupported => f.write_str("Updates can only be installed on Windows."),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<HttpError> for UpdateError {
    fn from(e: HttpError) -> Self {
        UpdateError::Network(e)
    }
}

impl From<io::Error> for UpdateError {
    fn from(e: io::Error) -> Self {
        UpdateError::Io(e)
    }
}

/// A newer version described by a validated manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Plain text, already trimmed and length-limited; safe to show as is.
    pub notes: String,
    url: String,
    sha256: [u8; 32],
}

/// Turn "2.1.10" into [2, 1, 10] so comparison is numeric, not alphabetical.
/// Lenient like the Python version: non-digits in a part are ignored.
pub fn parse_version(value: &str) -> Vec<u64> {
    value
        .split('.')
        .map(|part| {
            let digits: String = part.chars().filter(char::is_ascii_digit).take(18).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

/// True when `candidate` is a later version than `current`. Missing parts
/// count as zero, so "2.1" and "2.1.0" are equal.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (mut a, mut b) = (parse_version(candidate), parse_version(current));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

/// Strict check for a version that will be used in a file name: 1-4 numeric
/// parts, nothing else.
fn is_clean_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    (1..=4).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Validate a downloaded manifest against this app's rules.
pub fn parse_manifest(app: &AppInfo, text: &str) -> Result<Release, UpdateError> {
    let doc = json::parse(text).map_err(|_| UpdateError::BadManifest("not valid JSON"))?;
    let version = doc.get("version").and_then(json::Value::as_str).ok_or(UpdateError::BadManifest("no version"))?;
    let url = doc.get("url").and_then(json::Value::as_str).ok_or(UpdateError::BadManifest("no download address"))?;
    let digest = doc.get("sha256").and_then(json::Value::as_str).unwrap_or("").trim();
    let notes = doc.get("notes").and_then(json::Value::as_str).unwrap_or("");

    if !is_clean_version(version) {
        return Err(UpdateError::BadManifest("version is not in the form 1.2.3"));
    }
    if !is_allowed_download(app, url, version) {
        return Err(UpdateError::BadManifest("download address is not this program's release of that version"));
    }
    // The Python updater also refused to run anything without a checksum.
    let sha256 = parse_hex32(digest).ok_or(UpdateError::BadManifest("missing or malformed sha256"))?;

    let notes: String =
        notes.trim().chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(MAX_NOTES_CHARS).collect();
    Ok(Release { version: version.to_string(), notes, url: url.to_string(), sha256 })
}

/// `<download_prefix>v<version>/<file>.exe`, with tame characters only.
fn is_allowed_download(app: &AppInfo, url: &str, version: &str) -> bool {
    let Some(rest) = url.strip_prefix(app.download_prefix) else { return false };
    let segments: Vec<&str> = rest.split('/').collect();
    let tame = |s: &str| {
        !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
    };
    app.download_prefix.starts_with("https://")
        && segments.len() == 2
        && segments.iter().all(|s| tame(s))
        && segments[0] == format!("v{version}")
        && segments[1].to_ascii_lowercase().ends_with(".exe")
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

fn ensure_enabled(policy: &Policy) -> Result<(), UpdateError> {
    if let Some(problem) = &policy.load_error {
        return Err(UpdateError::PolicyUnreadable(problem.clone()));
    }
    if !policy.updates_enabled {
        return Err(UpdateError::DisabledByPolicy);
    }
    Ok(())
}

fn client(app: &AppInfo, policy: &Policy, step: Duration, total: Duration) -> Result<Client, UpdateError> {
    Ok(Client::new(http::Options {
        user_agent: app.user_agent.to_string(),
        proxy: policy.proxy.clone(),
        extra_root_certificates: policy.extra_root_certificates.clone(),
        connect_timeout: CHECK_STEP_TIMEOUT,
        io_timeout: step,
        total_timeout: total,
    })?)
}

/// Fetch the manifest and return the release if it is newer than this build.
///
/// Errors are returned rather than swallowed so a manual "Check for updates"
/// can say what went wrong; the automatic check at start-up should ignore
/// them, as the Python version did. Run this off the UI thread.
pub fn check(app: &AppInfo, policy: &Policy) -> Result<Option<Release>, UpdateError> {
    ensure_enabled(policy)?;
    let client = client(app, policy, CHECK_STEP_TIMEOUT, CHECK_TOTAL_TIMEOUT)?;
    let body = client.get(app.manifest_url, MAX_MANIFEST)?.read_to_end()?;
    let text = String::from_utf8(body).map_err(|_| UpdateError::BadManifest("not UTF-8 text"))?;
    let release = parse_manifest(app, &text)?;
    Ok(is_newer(&release.version, app.version).then_some(release))
}

/// Short text for the update prompt, tailored to how the program is installed.
pub fn install_hint(policy: &Policy) -> &'static str {
    match policy.scope {
        InstallScope::Machine => {
            "This program is installed for all users, so Windows will ask for administrator approval. \
             If you are not an administrator, ask IT to install the update."
        }
        _ => "The program will close briefly while the update installs.",
    }
}

/// A verified installer, locked against modification.
#[derive(Debug)]
pub struct Downloaded {
    folder: PathBuf,
    path: PathBuf,
    /// Held open with sharing that denies writes, renames and deletes (Windows).
    lock: File,
}

impl Downloaded {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Start the installer silently. `policy.scope` tells Setup which install
    /// to update when a machine has both a per-user and an all-users copy.
    ///
    /// Keep the returned value alive until the program exits (for example by
    /// calling [`Launched::wait`] on a background thread after hiding the
    /// window). Setup closes the program itself when it is ready to replace
    /// the files; until then the installer file stays locked.
    pub fn launch(self, policy: &Policy) -> Result<Launched, UpdateError> {
        let child = launch_installer(&self.path, &self.folder, policy.scope, true)?;
        Ok(Launched { child, _lock: self.lock })
    }

    /// As [`Downloaded::launch`], but Setup does not reopen the program
    /// afterwards (`/NORELAUNCH`): for updates started from a script.
    pub fn launch_without_relaunch(self, policy: &Policy) -> Result<Launched, UpdateError> {
        let child = launch_installer(&self.path, &self.folder, policy.scope, false)?;
        Ok(Launched { child, _lock: self.lock })
    }

    /// Delete the download without running it.
    pub fn discard(self) {
        let folder = self.folder.clone();
        drop(self);
        let _ = fs::remove_dir_all(folder);
    }
}

/// A running installer, with its file still locked.
#[derive(Debug)]
pub struct Launched {
    child: Child,
    _lock: File,
}

impl Launched {
    /// Block until Setup finishes (it usually closes this program first).
    /// Returns true if it reported success; false if it failed or the user
    /// declined the administrator prompt, in which case the program can carry
    /// on as before.
    pub fn wait(mut self) -> bool {
        self.child.wait().map(|s| s.success()).unwrap_or(false)
    }
}

/// Where downloads go: a folder in the user's own profile, which other users
/// cannot open. Falls back to %TEMP% only if the profile path is unknown.
fn downloads_root(app: &AppInfo) -> PathBuf {
    match paths::settings_dir(app.vendor, app.name) {
        Some(dir) => dir.join("updates"),
        None => std::env::temp_dir(),
    }
}

/// Download and verify the installer for `release`.
///
/// `progress(received, total)` is called as data arrives; return `false` from
/// it to cancel. Run this off the UI thread.
pub fn download(
    app: &AppInfo,
    policy: &Policy,
    release: &Release,
    mut progress: impl FnMut(u64, Option<u64>) -> bool,
) -> Result<Downloaded, UpdateError> {
    ensure_enabled(policy)?;
    cleanup_stale_downloads(app);

    let root = downloads_root(app);
    fs::create_dir_all(&root)?;
    // A new folder with a random name. create_dir (not create_dir_all) fails
    // if the name already exists, so nothing can be planted in advance.
    let folder = loop {
        let candidate = root.join(format!("{}-update-{}", app.installer_stem, fsutil::random_token()));
        match create_private_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    // `is_clean_version` guarantees digits and dots only.
    let path = folder.join(format!("{}_{}.exe", app.installer_stem, release.version));

    match fetch_and_verify(app, policy, release, &path, &mut progress) {
        Ok(lock) => Ok(Downloaded { folder, path, lock }),
        Err(e) => {
            let _ = fs::remove_dir_all(&folder);
            Err(e)
        }
    }
}

fn fetch_and_verify(
    app: &AppInfo,
    policy: &Policy,
    release: &Release,
    path: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<File, UpdateError> {
    let client = client(app, policy, DOWNLOAD_STEP_TIMEOUT, DOWNLOAD_TOTAL_TIMEOUT)?;
    let mut response = client.get(&release.url, MAX_INSTALLER)?;
    let total = response.content_length;

    {
        let mut out = exclusive_create(path)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut received = 0u64;
        if !progress(0, total) {
            return Err(UpdateError::Cancelled);
        }
        loop {
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            received += n as u64;
            if !progress(received, total) {
                return Err(UpdateError::Cancelled);
            }
        }
        out.sync_all()?;
    }

    // Reopen read-only, denying other writers, and hash what is on disk: that
    // is exactly what will be executed.
    let mut lock = open_locked(path)?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = lock.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    if digest.finish().as_ref() != release.sha256 {
        return Err(UpdateError::ChecksumMismatch);
    }
    Ok(lock)
}

/// Remove download folders left by earlier updates (an installer cannot
/// delete itself while running). Best effort, and only our own prefix is
/// touched. Safe to call at start-up.
pub fn cleanup_stale_downloads(app: &AppInfo) {
    let prefix = format!("{}-update-", app.installer_stem);
    let now = SystemTime::now();
    // %TEMP% too, for folders left by builds that downloaded there.
    for root in [downloads_root(app), std::env::temp_dir()] {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            // symlink_metadata: a link is never followed, and never matches is_dir.
            let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
            let old =
                meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age > STALE_AFTER);
            if meta.is_dir() && old {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------- platform parts

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const FILE_SHARE_READ: u32 = 0x1;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const ERROR_ELEVATION_REQUIRED: i32 = 740;

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        // Inside the user's profile the inherited ACL limits access to this
        // user, SYSTEM and administrators.
        fs::create_dir(path)
    }

    pub fn exclusive_create(path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).share_mode(0).open(path)
    }

    pub fn open_locked(path: &Path) -> io::Result<File> {
        // Others may read (Windows needs to, to start it) but not write,
        // rename or delete while we hold this handle.
        OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path)
    }

    pub fn launch_installer(
        path: &Path,
        folder: &Path,
        scope: InstallScope,
        relaunch: bool,
    ) -> Result<Child, UpdateError> {
        // /SILENT shows only a progress bar. Inno's CloseApplications shuts
        // this program down when it is ready. Explicit null handles: a
        // windowed program has no console to pass on.
        let mut cmd = Command::new(path);
        cmd.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
        if !relaunch {
            cmd.arg("/NORELAUNCH");
        }
        match scope {
            InstallScope::Machine => {
                cmd.arg("/ALLUSERS");
            }
            InstallScope::User => {
                cmd.arg("/CURRENTUSER");
            }
            InstallScope::Unknown => {}
        }
        let spawned = cmd
            .current_dir(folder)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => Ok(child),
            Err(e) if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => Err(UpdateError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the installer requires administrator rights; ask IT to install the update",
            ))),
            Err(e) => Err(UpdateError::Io(e)),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    pub fn create_private_dir(path: &Path) -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(path)
    }

    pub fn exclusive_create(path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
    }

    pub fn open_locked(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    pub fn launch_installer(
        _path: &Path,
        _folder: &Path,
        _scope: InstallScope,
        _relaunch: bool,
    ) -> Result<Child, UpdateError> {
        Err(UpdateError::Unsupported)
    }
}

use platform::{create_private_dir, exclusive_create, launch_installer, open_locked};

#[cfg(test)]
mod tests {
    use super::*;

    const APP: AppInfo = AppInfo {
        vendor: "V",
        name: "N",
        version: "3.0.0",
        config_schema: 1,
        user_agent: "test",
        manifest_url: "https://github.com/o/r/releases/latest/download/version.json",
        download_prefix: "https://github.com/o/r/releases/download/",
        installer_stem: "n_setup",
    };

    fn manifest(version: &str, url: &str, sha: &str) -> String {
        format!(r#"{{"version": "{version}", "url": "{url}", "sha256": "{sha}", "notes": "Fixes.\u0007"}}"#)
    }

    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const URL: &str = "https://github.com/o/r/releases/download/v3.0.1/App-3.0.1-setup.exe";

    #[test]
    fn versions() {
        assert!(is_newer("2.1.10", "2.1.9"));
        assert!(!is_newer("2.1", "2.1.0"));
        assert!(!is_newer("2.1.0", "2.1"));
        assert!(is_newer("3.0.0", "2.1.2"));
        assert!(!is_newer("garbage", "1.0"));
        assert!(is_clean_version("3.0.1") && is_clean_version("10") && !is_clean_version("3.0.1-beta"));
        assert!(!is_clean_version("..\\..\\x") && !is_clean_version("1.2.3.4.5") && !is_clean_version(""));
    }

    #[test]
    fn manifest_validation() {
        let r = parse_manifest(&APP, &manifest("3.0.1", URL, SHA)).unwrap();
        assert_eq!(r.version, "3.0.1");
        assert_eq!(r.notes, "Fixes.");
        let upper = SHA.to_ascii_uppercase();
        assert!(parse_manifest(&APP, &manifest("3.0.1", URL, &upper)).is_ok());

        for (v, u, s) in [
            ("3.0.1/../../x", URL, SHA),
            ("3.0.1", "http://github.com/o/r/releases/download/v3.0.1/App.exe", SHA),
            ("3.0.1", "file:///C:/Windows/System32/calc.exe", SHA),
            ("3.0.1", "https://evil.example/o/r/releases/download/v3.0.1/App.exe", SHA),
            ("3.0.1", "https://github.com/o/r/releases/download/v3.0.1/../../x/App.exe", SHA),
            ("3.0.1", "https://github.com/o/r/releases/download/v3.0.1/App.msi", SHA),
            ("3.0.1", "https://github.com/o/r/releases/download/App.exe", SHA),
            ("3.0.1", "https://github.com/o/r/releases/download/v3.0.1/App.exe?x=1", SHA),
            ("3.0.2", URL, SHA), // version does not match the release tag
            ("3.0.1", URL, ""),
            ("3.0.1", URL, "abc"),
            ("3.0.1", URL, &SHA.replace('9', "g")),
        ] {
            assert!(parse_manifest(&APP, &manifest(v, u, s)).is_err(), "{v} {u} {s}");
        }
        assert!(parse_manifest(&APP, "[]").is_err());
        assert!(parse_manifest(&APP, r#"{"version": 3, "url": "x"}"#).is_err());
    }

    #[test]
    fn locked_download_cannot_be_modified() {
        let dir = std::env::temp_dir().join(format!("upd-lock-{}", fsutil::random_token()));
        create_private_dir(&dir).unwrap();
        let path = dir.join("setup.exe");
        exclusive_create(&path).unwrap().write_all(b"MZ").unwrap();
        assert!(exclusive_create(&path).is_err(), "create_new must refuse an existing file");
        let lock = open_locked(&path).unwrap();
        let tamper = OpenOptions::new().write(true).open(&path);
        let rename = fs::rename(&path, dir.join("moved.exe"));
        let delete = fs::remove_file(&path);
        if cfg!(windows) {
            assert!(tamper.is_err(), "writing must be refused while locked");
            assert!(rename.is_err(), "renaming must be refused while locked");
            assert!(delete.is_err(), "deleting must be refused while locked");
        }
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_by_policy() {
        let policy = Policy { updates_enabled: false, ..Policy::default() };
        assert!(matches!(check(&APP, &policy), Err(UpdateError::DisabledByPolicy)));
        let unreadable = Policy { load_error: Some("bad".into()), ..Policy::default() };
        assert!(matches!(check(&APP, &unreadable), Err(UpdateError::PolicyUnreadable(_))));
    }
}
