//! Machine-level settings an administrator controls: `policy.ini` beside the
//! executable.
//!
//! A file rather than the registry keeps advapi32.dll out of the process. In
//! a machine-wide install the program folder is under Program Files, so only
//! administrators can change it. In a per-user install the user owns the
//! folder anyway, so the file grants nothing they could not already do.
//!
//! ```ini
//! [Install]
//! ; Written by the installer: "machine" (all users) or "user".
//! Scope=machine
//!
//! [Updates]
//! ; 0 turns the update check off (for fleets where IT deploys versions).
//! Enabled=1
//! ; Optional HTTP proxy for the update check. Otherwise HTTPS_PROXY is used.
//! Proxy=http://proxy.example.com:8080
//! ; Optional PEM file of extra trusted root certificates, for networks that
//! ; inspect TLS. Relative paths are resolved against the program folder.
//! ExtraRootCertificates=corp-root.pem
//! ```

use std::path::{Path, PathBuf};

use super::fsutil;

pub const FILE_NAME: &str = "policy.ini";
const MAX_SIZE: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallScope {
    /// Installed for all users (normally under Program Files). Installing an
    /// update needs administrator approval.
    Machine,
    /// Installed for the current user only.
    User,
    /// No installer marker: a development build or a copied exe.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub scope: InstallScope,
    pub updates_enabled: bool,
    pub proxy: Option<String>,
    pub extra_root_certificates: Option<PathBuf>,
    /// Set when policy.ini exists but could not be read. The updater then
    /// stays off: an administrator who wrote the file most likely meant to
    /// restrict something.
    pub load_error: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            scope: InstallScope::Unknown,
            updates_enabled: true,
            proxy: None,
            extra_root_certificates: None,
            load_error: None,
        }
    }
}

impl Policy {
    /// Load `policy.ini` from the program folder. A missing or unreadable file
    /// gives the defaults.
    pub fn load() -> Policy {
        match super::paths::program_dir() {
            Some(dir) => Policy::load_from(&dir),
            None => Policy::default(),
        }
    }

    pub fn load_from(program_dir: &Path) -> Policy {
        match fsutil::read_capped(&program_dir.join(FILE_NAME), MAX_SIZE) {
            Ok(bytes) => Policy::parse(&decode(&bytes), program_dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Policy::default(),
            Err(e) => Policy { load_error: Some(e.to_string()), ..Policy::default() },
        }
    }

    pub fn parse(text: &str, program_dir: &Path) -> Policy {
        let mut policy = Policy::default();
        for (section, key, value) in ini_entries(text) {
            match (section.as_str(), key.as_str()) {
                ("install", "scope") => {
                    policy.scope = match value.to_ascii_lowercase().as_str() {
                        "machine" | "allusers" | "admin" => InstallScope::Machine,
                        "user" | "currentuser" => InstallScope::User,
                        _ => InstallScope::Unknown,
                    }
                }
                ("updates", "enabled") => {
                    // Anything but an explicit "off" leaves updates on, so a
                    // typo cannot silently stop security fixes arriving.
                    policy.updates_enabled =
                        !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
                }
                ("updates", "proxy") if !value.is_empty() => policy.proxy = Some(value),
                ("updates", "extrarootcertificates") if !value.is_empty() => {
                    let path = PathBuf::from(&value);
                    policy.extra_root_certificates =
                        Some(if path.is_absolute() { path } else { program_dir.join(path) });
                }
                _ => {}
            }
        }
        policy
    }
}

/// Text from UTF-8 or UTF-16 (Notepad's "Unicode", PowerShell 5's default
/// for `>` and Out-File), with or without a byte-order mark.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |data: &[u8], big_endian: bool| {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| if big_endian { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        // No BOM but every other byte zero: UTF-16 little-endian ASCII text.
        _ if bytes.len() >= 4 && bytes.iter().skip(1).step_by(2).all(|&b| b == 0) => utf16(bytes, false),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// (lower-case section, lower-case key, trimmed value) for each `key=value`.
fn ini_entries(text: &str) -> Vec<(String, String, String)> {
    let mut section = String::new();
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"').to_string();
            out.push((section.clone(), key.trim().to_ascii_lowercase(), value));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_policy() {
        let dir = Path::new(if cfg!(windows) { r"C:\Program Files\App" } else { "/opt/app" });
        let p = Policy::parse(
            "\u{feff}; comment\n[Install]\nScope = Machine\n\n[UPDATES]\nEnabled=0\nProxy=http://p:8080\nExtraRootCertificates=\"corp.pem\"\n",
            dir,
        );
        assert_eq!(p.scope, InstallScope::Machine);
        assert!(!p.updates_enabled);
        assert_eq!(p.proxy.as_deref(), Some("http://p:8080"));
        assert_eq!(p.extra_root_certificates, Some(dir.join("corp.pem")));

        let typo = Policy::parse("[Updates]\nEnabled=nope\n", dir);
        assert!(typo.updates_enabled);
        assert_eq!(typo.scope, InstallScope::Unknown);
    }

    #[test]
    fn reads_utf16_files() {
        let text = "[Updates]\r\nEnabled=0\r\n";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        let bare: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        for bytes in [le, be, bare] {
            let dir = std::env::temp_dir().join(format!("policy-{}", fsutil::random_token()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(FILE_NAME), &bytes).unwrap();
            assert!(!Policy::load_from(&dir).updates_enabled);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
