//! Showing files to the user.
//!
//! On Windows this starts explorer.exe instead of calling ShellExecute, so
//! shell32.dll is never loaded into this process. Explorer applies the same
//! file associations ShellExecute would. If application control restricts
//! which programs this app may start, explorer.exe must be allowed.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Open a file in its default program (e.g. an image viewer).
pub fn open(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    platform::open(&path)
}

/// Open the folder containing `path` with the item selected.
pub fn reveal(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    platform::reveal(&path)
}

fn spawn_detached(mut command: Command) -> io::Result<()> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // The child is not waited for; dropping the handle does not stop it.
    command.spawn().map(drop)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Full path to explorer.exe, so the search path is never consulted.
    fn explorer() -> PathBuf {
        let windows = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        windows.join("explorer.exe")
    }

    fn check(path: &Path) -> io::Result<()> {
        // Explorer cannot take a quote inside a quoted argument. Windows file
        // names cannot contain one either, so this only rejects garbage.
        if path.as_os_str().to_string_lossy().contains('"') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid path"));
        }
        Ok(())
    }

    pub fn open(path: &Path) -> io::Result<()> {
        check(path)?;
        let mut cmd = Command::new(explorer());
        // Always quoted: Explorer treats an unquoted comma as a separator,
        // so "Smith,John.png" would not open otherwise.
        cmd.raw_arg(format!("\"{}\"", path.display())).creation_flags(CREATE_NO_WINDOW);
        spawn_detached(cmd)
    }

    pub fn reveal(path: &Path) -> io::Result<()> {
        check(path)?;
        let mut cmd = Command::new(explorer());
        // Explorer parses "/select,<path>" itself, so it must be passed
        // verbatim rather than through the standard argument quoting.
        cmd.raw_arg(format!("/select,\"{}\"", path.display())).creation_flags(CREATE_NO_WINDOW);
        spawn_detached(cmd)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn open(path: &Path) -> io::Result<()> {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("--").arg(path);
        spawn_detached(cmd)
    }

    pub fn reveal(path: &Path) -> io::Result<()> {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-R").arg("--").arg(path);
        spawn_detached(cmd)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    pub fn open(path: &Path) -> io::Result<()> {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        spawn_detached(cmd)
    }

    pub fn reveal(path: &Path) -> io::Result<()> {
        let folder = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        open(folder)
    }
}
