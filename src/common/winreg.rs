//! Read-only registry access through the `api-ms-win-core-registry` API set.
//!
//! API sets are virtual DLL names that Windows resolves to kernelbase.dll,
//! which every process already has loaded, so this adds no DLL load
//! (advapi32.dll is never touched).

use std::ffi::c_void;

type Hkey = *mut c_void;
const HKEY_CURRENT_USER: Hkey = 0x8000_0001_usize as Hkey;
const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002_usize as Hkey;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_WOW64_64KEY: u32 = 0x0100;
const KEY_WOW64_32KEY: u32 = 0x0200;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_DWORD: u32 = 4;

#[link(name = "api-ms-win-core-registry-l1-1-0", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, subkey: *const u16, options: u32, sam: u32, result: *mut Hkey) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    CurrentUser,
    LocalMachine,
}

/// Which registry view to open: 64-bit programs' keys or 32-bit programs'
/// (`WOW6432Node`). Only matters under HKLM\Software.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Native64,
    Wow32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// An open key, closed on drop.
pub struct Key(Hkey);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    /// Open `path` for reading, or `None` if it does not exist.
    pub fn open(root: Root, path: &str, view: View) -> Option<Key> {
        let root = match root {
            Root::CurrentUser => HKEY_CURRENT_USER,
            Root::LocalMachine => HKEY_LOCAL_MACHINE,
        };
        let sam = KEY_QUERY_VALUE | if view == View::Wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY };
        let path = wide(path);
        let mut raw: Hkey = std::ptr::null_mut();
        // SAFETY: predefined root key, NUL-terminated path, valid out pointer.
        let rc = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, sam, &mut raw) };
        (rc == 0).then_some(Key(raw))
    }

    /// (type, bytes) of a value, capped at 64 KiB.
    fn query(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let name = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: valid key handle, NUL-terminated name, null data pointer to
        // ask for the size.
        let rc = unsafe {
            RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut len)
        };
        if rc != 0 || len > 64 * 1024 {
            return None;
        }
        let mut data = vec![0u8; len as usize];
        // SAFETY: `data` holds `len` writable bytes.
        let rc = unsafe {
            RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, data.as_mut_ptr(), &mut len)
        };
        if rc != 0 {
            return None;
        }
        data.truncate(len as usize);
        Some((kind, data))
    }

    /// A REG_SZ or REG_EXPAND_SZ value (not expanded).
    pub fn string(&self, name: &str) -> Option<String> {
        match self.query(name)? {
            (REG_SZ | REG_EXPAND_SZ, bytes) => {
                let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                Some(String::from_utf16_lossy(&units[..end]))
            }
            _ => None,
        }
    }

    pub fn dword(&self, name: &str) -> Option<u32> {
        match self.query(name)? {
            (REG_DWORD, b) if b.len() >= 4 => Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            _ => None,
        }
    }
}
