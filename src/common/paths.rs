//! Well-known folders, found the same way the Python builds found them.
//!
//! Environment variables are used instead of the Windows shell API
//! (SHGetKnownFolderPath) so that shell32.dll and ole32.dll are never loaded.
//! Windows sets these variables for every logon session.

use std::env;
use std::path::{Path, PathBuf};

/// Read an environment variable as an absolute path. Empty or relative values
/// are ignored, so a bad variable can never make us write relative to the
/// current directory.
fn env_dir(name: &str) -> Option<PathBuf> {
    let value = env::var_os(name)?;
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return None;
    }
    Some(path)
}

/// The user's profile folder (`%USERPROFILE%`, or `$HOME` elsewhere).
pub fn home() -> Option<PathBuf> {
    #[cfg(windows)]
    let found = env_dir("USERPROFILE").or_else(|| {
        let drive = env::var_os("HOMEDRIVE")?;
        let path = env::var_os("HOMEPATH")?;
        let mut joined = PathBuf::from(drive);
        joined.push(path);
        joined.is_absolute().then_some(joined)
    });
    #[cfg(not(windows))]
    let found = env_dir("HOME");
    found
}

/// Root for per-user application data: `%LOCALAPPDATA%` on Windows, the XDG
/// config folder elsewhere (non-Windows builds exist for development).
pub fn local_app_data() -> Option<PathBuf> {
    #[cfg(windows)]
    let found = env_dir("LOCALAPPDATA").or_else(|| Some(home()?.join("AppData").join("Local")));
    #[cfg(not(windows))]
    let found = env_dir("XDG_CONFIG_HOME").or_else(|| Some(home()?.join(".config")));
    found
}

/// Per-user settings folder: `<local app data>\<vendor>\<app name>`.
pub fn settings_dir(vendor: &str, app_name: &str) -> Option<PathBuf> {
    Some(local_app_data()?.join(vendor).join(app_name))
}

/// The user's Downloads folder, as `home\Downloads` (what the Python build
/// used). Falls back to the temporary folder, which is always absolute.
pub fn downloads() -> PathBuf {
    match home() {
        Some(h) => h.join("Downloads"),
        None => env::temp_dir(),
    }
}

/// The folder holding the running executable.
pub fn program_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    exe.parent().map(Path::to_path_buf)
}

/// Nearest existing ancestor of `path` (or `path` itself), for pointing a file
/// picker somewhere sensible when the saved folder has been deleted.
pub fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.is_dir() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}
