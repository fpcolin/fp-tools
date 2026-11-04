//! The separate QR Code Generator and VendorCafe CSV Converter apps that this
//! program replaces.
//!
//! Their installers were per-user (Inno Setup, `PrivilegesRequired=lowest`),
//! so each person who used them has their own copy, registered under
//! HKEY_CURRENT_USER. This program's installer usually runs as an
//! administrator and cannot see those copies; instead, the program finds the
//! current user's copies after copying their settings, and offers to remove
//! them by running their own uninstallers. Copies installed for all users
//! (HKEY_LOCAL_MACHINE) are removed by this program's installer.

use std::io;
use std::path::PathBuf;

/// Inno Setup AppIds of the old apps. Never reuse these.
pub const OLD_APPS: [(&str, &str); 2] = [
    ("QR Code Generator", "{0234F8BE-94D5-4A96-8387-A0CB5A5DAD1B}"),
    ("VendorCafe CSV Converter", "{AB4F23FC-7020-4D60-9CFC-3EB453F9A9BA}"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldApp {
    /// "QR Code Generator" or "VendorCafe CSV Converter".
    pub name: &'static str,
    /// Installed for all users: only an administrator can remove it.
    pub all_users: bool,
    /// Its uninstaller, if the registration names a plausible one.
    pub uninstaller: Option<PathBuf>,
}

impl OldApp {
    /// Removable by this user from inside the program.
    pub fn removable(&self) -> bool {
        !self.all_users && self.uninstaller.is_some()
    }
}

/// The old apps that are still installed, current user's copies first.
pub fn find() -> Vec<OldApp> {
    imp::find()
}

/// Run an old app's uninstaller silently and wait for it. Returns `Ok(true)`
/// when the app is gone afterwards.
pub fn remove(app: &OldApp) -> io::Result<bool> {
    let Some(uninstaller) = app.uninstaller.as_ref().filter(|_| !app.all_users) else {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "an administrator has to remove this copy"));
    };
    let status =
        std::process::Command::new(uninstaller).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]).status()?;
    let still_there = find().iter().any(|a| a.name == app.name && !a.all_users);
    Ok(status.success() && !still_there)
}

/// The uninstaller named by an Inno Setup `UninstallString`, accepted only if
/// it looks like one: an absolute path to an existing `unins###.exe`. The
/// value comes from the user's own registry hive, so this is a sanity check
/// against running something unexpected, not a security boundary.
pub fn parse_uninstaller(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let path = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split(" /").next()?.trim(),
    };
    let path = PathBuf::from(path);
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let plausible = name.len() == 12
        && name.starts_with("unins")
        && name.ends_with(".exe")
        && name.as_bytes()[5..8].iter().all(u8::is_ascii_digit);
    (plausible && path.is_absolute()).then_some(path)
}

#[cfg(windows)]
mod imp {
    use super::{OLD_APPS, OldApp, parse_uninstaller};
    use crate::common::winreg::{Key, Root, View};

    pub fn find() -> Vec<OldApp> {
        let mut found = Vec::new();
        for (root, all_users) in [(Root::CurrentUser, false), (Root::LocalMachine, true)] {
            for (name, id) in OLD_APPS {
                let path = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{id}_is1");
                for view in [View::Native64, View::Wow32] {
                    let Some(key) = Key::open(root, &path, view) else { continue };
                    let uninstaller =
                        key.string("UninstallString").as_deref().and_then(parse_uninstaller).filter(|p| p.is_file());
                    let app = OldApp { name, all_users, uninstaller };
                    if !found.contains(&app) {
                        found.push(app);
                    }
                    // HKCU\Software is shared by both views.
                    if root == Root::CurrentUser {
                        break;
                    }
                }
            }
        }
        found
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn find() -> Vec<super::OldApp> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_strings() {
        let p =
            if cfg!(windows) { r"C:\Users\a\AppData\Local\Programs\FP\unins000.exe" } else { "/opt/fp/unins000.exe" };
        assert_eq!(parse_uninstaller(&format!("\"{p}\"")), Some(PathBuf::from(p)));
        assert_eq!(parse_uninstaller(&format!("\"{p}\" /SILENT")), Some(PathBuf::from(p)));
        assert_eq!(parse_uninstaller(&format!("{p} /SILENT")), Some(PathBuf::from(p)));
        assert_eq!(parse_uninstaller("\"C:\\x\\cmd.exe\" /c del"), None);
        assert_eq!(parse_uninstaller("\"unins000.exe\""), None);
        assert_eq!(parse_uninstaller("\"C:\\x\\uninsABC.exe\""), None);
    }
}
