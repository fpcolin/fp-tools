//! The separate QR Code Generator and VendorCafe CSV Converter apps that this
//! program replaces.
//!
//! Their installers were per-user (Inno Setup, `PrivilegesRequired=lowest`),
//! so each person who used them has their own copy, registered under
//! HKEY_CURRENT_USER. This program's installer usually runs as an
//! administrator and cannot see those copies; instead, the first time a
//! person opens this program it copies their settings and then removes their
//! copies by running the old apps' own uninstallers. (The order matters: an
//! old uninstaller deletes that app's settings folder.) Copies installed for
//! all users (HKEY_LOCAL_MACHINE) are removed by this program's installer.

use std::io;
use std::path::PathBuf;

/// Name, Inno Setup AppId and program file of each old app. Never reuse the
/// AppIds.
pub const OLD_APPS: [(&str, &str, &str); 2] = [
    ("QR Code Generator", "{0234F8BE-94D5-4A96-8387-A0CB5A5DAD1B}", "FPQRGenerator.exe"),
    ("VendorCafe CSV Converter", "{AB4F23FC-7020-4D60-9CFC-3EB453F9A9BA}", "FPVendorCafeConverter.exe"),
];

/// How long to wait for an old app that is still open to close (it is
/// usually the one that just started this program's installer, and is on its
/// way out) before giving up on removing it this time.
const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

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
/// when the app is gone afterwards. If the app is still open in this
/// Windows session, waits up to 30 seconds for it to close, then fails with
/// [`io::ErrorKind::ResourceBusy`] without touching it.
pub fn remove(app: &OldApp) -> io::Result<bool> {
    let Some(uninstaller) = app.uninstaller.as_ref().filter(|_| !app.all_users) else {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "an administrator has to remove this copy"));
    };
    let exe = OLD_APPS.iter().find(|(name, _, _)| *name == app.name).map(|(_, _, exe)| *exe);
    if let Some(exe) = exe {
        let start = std::time::Instant::now();
        while imp::is_running(exe) {
            if start.elapsed() >= CLOSE_WAIT {
                return Err(io::Error::new(io::ErrorKind::ResourceBusy, "it is still open"));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
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
            for (name, id, _) in OLD_APPS {
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

    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        priority: i32,
        flags: u32,
        exe_file: [u16; 260],
    }

    const TH32CS_SNAPPROCESS: u32 = 0x2;
    const INVALID_HANDLE_VALUE: isize = -1;

    // kernel32 is loaded into every process; these add no DLL.
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn ProcessIdToSessionId(process_id: u32, session_id: *mut u32) -> i32;
        fn GetCurrentProcessId() -> u32;
    }

    /// Is a program with this file name running in this Windows session?
    /// (On a shared Remote Desktop server, other people's copies don't count.)
    pub fn is_running(exe: &str) -> bool {
        let mut my_session = 0u32;
        // SAFETY: valid out pointer.
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut my_session) } == 0 {
            return false;
        }
        // SAFETY: plain snapshot handle, closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot == 0 {
            return false;
        }
        // SAFETY: the struct is plain data; dwSize is set as the API requires.
        let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut found = false;
        // SAFETY: valid snapshot handle and entry pointer.
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while ok {
            let len = entry.exe_file.iter().position(|&c| c == 0).unwrap_or(entry.exe_file.len());
            let name = String::from_utf16_lossy(&entry.exe_file[..len]);
            if name.eq_ignore_ascii_case(exe) {
                let mut session = u32::MAX;
                // SAFETY: valid out pointer.
                let known = unsafe { ProcessIdToSessionId(entry.process_id, &mut session) } != 0;
                if known && session == my_session {
                    found = true;
                    break;
                }
            }
            // SAFETY: as above.
            ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        // SAFETY: closing the snapshot we opened.
        unsafe { CloseHandle(snapshot) };
        found
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn find() -> Vec<super::OldApp> {
        Vec::new()
    }

    pub fn is_running(_exe: &str) -> bool {
        false
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
