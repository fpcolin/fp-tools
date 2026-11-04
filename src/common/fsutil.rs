//! File-system helpers that stay safe when several users or several copies of
//! the program share a folder (network shares, C:\Users\Public, a team
//! OneDrive folder...).
//!
//! The rules they enforce:
//! * Never write through a name someone else could have prepared: temporary
//!   and output files are created with "create new" semantics, which fails if
//!   anything (a file, a link, a folder) already exists at that name.
//! * Never leave a half-written file under the real name: write a temporary
//!   sibling, then rename it over the target in one step.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 16 hex characters from the system CSPRNG, for unguessable file names.
pub fn random_token() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 8];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        // ProcessPrng is documented never to fail. If it somehow did, fall
        // back to the std hasher's per-process random keys plus the clock:
        // still unpredictable to another user, which is all a name needs.
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        h.write_u32(std::process::id());
        bytes = h.finish().to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read a whole file, refusing anything larger than `max_bytes`.
pub fn read_capped(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file is too large"));
    }
    let mut data = Vec::with_capacity(len as usize);
    // take() guards against the file growing after the size check.
    file.take(max_bytes + 1).read_to_end(&mut data)?;
    if data.len() as u64 > max_bytes {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file is too large"));
    }
    Ok(data)
}

/// Create `<stem><suffix>` in `folder`, or `<stem> (1)<suffix>`, `(2)`... if
/// taken. The file is created atomically, so two people saving to the same
/// shared folder at the same moment get different names instead of one
/// overwriting the other.
pub fn create_unique(folder: &Path, stem: &str, suffix: &str) -> io::Result<(PathBuf, File)> {
    for counter in 0u32..10_000 {
        let name = if counter == 0 { format!("{stem}{suffix}") } else { format!("{stem} ({counter}){suffix}") };
        let candidate = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // Windows reports "access denied" rather than "exists" when the
            // name belongs to a folder or a file pending deletion.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && fs::symlink_metadata(&candidate).is_ok() => {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "too many files with the same name in this folder"))
}

/// Replace `target` with what `write` produces, atomically: readers see either
/// the old file or the complete new one, never a partial write.
pub fn replace_file_with<F>(target: &Path, write: F) -> io::Result<()>
where
    F: FnOnce(&mut dyn Write) -> io::Result<()>,
{
    let folder = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if target.file_name().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "target has no file name"));
    }

    // The temporary name does not include the target's, so a target name near
    // Windows' 255-character limit still works.
    let (temp, file) = loop {
        let candidate = folder.join(format!(".{}.tmp", random_token()));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };

    let result = (|| {
        let mut out = BufWriter::new(&file);
        write(&mut out)?;
        out.flush()?;
        drop(out);
        file.sync_all()?;
        drop(file);
        rename_with_retry(&temp, target)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Write bytes to `target` atomically.
pub fn write_atomic(target: &Path, data: &[u8]) -> io::Result<()> {
    replace_file_with(target, |w| w.write_all(data))
}

/// Rename, retrying briefly on Windows sharing violations. Antivirus and
/// search indexers often hold a freshly written file open for a moment.
fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 4 && is_transient_lock(&e) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(60 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_transient_lock(e: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33)
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fsutil-test-{name}-{}", random_token()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unique_names_never_collide() {
        let dir = scratch("unique");
        let (a, _) = create_unique(&dir, "qr_code", ".png").unwrap();
        let (b, _) = create_unique(&dir, "qr_code", ".png").unwrap();
        fs::create_dir(dir.join("qr_code (2).png")).unwrap();
        let (c, _) = create_unique(&dir, "qr_code", ".png").unwrap();
        assert_eq!(a.file_name().unwrap(), "qr_code.png");
        assert_eq!(b.file_name().unwrap(), "qr_code (1).png");
        assert_eq!(c.file_name().unwrap(), "qr_code (3).png");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replace_is_atomic_and_cleans_up() {
        let dir = scratch("replace");
        let target = dir.join("out.csv");
        write_atomic(&target, b"one").unwrap();
        write_atomic(&target, b"two").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        let err = replace_file_with(&target, |_| Err(io::Error::other("boom")));
        assert!(err.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temporary file was not removed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 16);
    }
}
