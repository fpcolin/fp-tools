//! The standard Open and Save As dialogs (comdlg32).

use std::path::{Path, PathBuf};

use super::win32::*;

/// Up to 32k characters, so long paths work.
const BUFFER: usize = 32 * 1024;

/// "Name\0*.ext;*.ext\0...\0\0"
fn filter(entries: &[(&str, &str)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (name, pattern) in entries {
        out.extend(name.encode_utf16());
        out.push(0);
        out.extend(pattern.encode_utf16());
        out.push(0);
    }
    out.push(0);
    out
}

fn run(
    owner: HWND,
    title: &str,
    filters: &[(&str, &str)],
    initial_dir: Option<&Path>,
    initial_name: &str,
    default_ext: Option<&str>,
    save: bool,
) -> Option<PathBuf> {
    let filter = filter(filters);
    let title = wide(title);
    let dir = initial_dir.filter(|d| d.is_dir()).map(|d| wide(&d.to_string_lossy()));
    let ext = default_ext.map(wide);
    let mut file = vec![0u16; BUFFER];
    for (slot, unit) in file.iter_mut().zip(initial_name.encode_utf16().take(BUFFER - 1)) {
        *slot = unit;
    }
    let flags = OFN_EXPLORER
        | OFN_HIDEREADONLY
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | OFN_DONTADDTORECENT
        | if save { OFN_NOREADONLYRETURN } else { OFN_FILEMUSTEXIST };
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        hInstance: std::ptr::null_mut(),
        lpstrFilter: filter.as_ptr(),
        lpstrCustomFilter: std::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: BUFFER as u32,
        lpstrFileTitle: std::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
        lpstrTitle: title.as_ptr(),
        Flags: flags,
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: ext.as_ref().map_or(std::ptr::null(), |e| e.as_ptr()),
        lCustData: 0,
        lpfnHook: std::ptr::null(),
        lpTemplateName: std::ptr::null(),
        pvReserved: std::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };
    // SAFETY: every pointer in `ofn` refers to a live buffer above, and the
    // file buffer's size is given in nMaxFile.
    let ok = unsafe { if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) } };
    if ok == 0 {
        return None;
    }
    let end = file.iter().position(|&u| u == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..end])))
}

/// Ask for an existing file.
pub fn open_file(owner: HWND, title: &str, filters: &[(&str, &str)], current: Option<&Path>) -> Option<PathBuf> {
    let dir = current.and_then(Path::parent);
    run(owner, title, filters, dir, "", None, false)
}

/// Ask where to save, starting in `dir` with `name` filled in.
pub fn save_file(
    owner: HWND,
    title: &str,
    filters: &[(&str, &str)],
    dir: &Path,
    name: &str,
    default_ext: &str,
) -> Option<PathBuf> {
    run(owner, title, filters, Some(dir), name, Some(default_ext), true)
}
