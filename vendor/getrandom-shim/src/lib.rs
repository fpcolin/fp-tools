//! Minimal replacement for the `getrandom` 0.2 crate.
//!
//! Why this exists: `ring` (used by rustls for the HTTPS update check) gets its
//! random numbers from `getrandom` 0.2, which on Windows imports
//! `BCryptGenRandom` from bcrypt.dll and `SystemFunction036` from
//! advapi32.dll. Application-control products such as ThreatLocker can require
//! a policy for every DLL a program loads, so those two imports are pure cost.
//!
//! The Rust standard library already imports `ProcessPrng` from
//! bcryptprimitives.dll (it seeds `HashMap` with it), and `ProcessPrng` is the
//! system CSPRNG that `BCryptGenRandom` itself is built on. Routing `ring`
//! through it adds no new DLL. `getrandom` 0.3 made the same switch upstream;
//! this shim can be deleted once `ring` moves to 0.3.
//!
//! It is wired in with `[patch.crates-io]` in the top-level Cargo.toml, and
//! the release workflow checks the built exe's import table so a dependency
//! update cannot quietly bring the extra DLLs back.
//!
//! Only the API `ring` uses is provided: `getrandom()` and `Error`.

#![no_std]

use core::fmt;
use core::num::NonZeroU32;

/// Opaque error, mirroring `getrandom::Error`.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Error(NonZeroU32);

impl Error {
    /// The platform RNG reported a failure.
    pub const UNEXPECTED: Error = Error(match NonZeroU32::new(0x8000_0001) {
        Some(code) => code,
        None => unreachable!(),
    });

    pub fn code(self) -> NonZeroU32 {
        self.0
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "getrandom::Error({:#x})", self.0.get())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the system random number generator failed")
    }
}

/// Fill `dest` with cryptographically secure random bytes.
pub fn getrandom(dest: &mut [u8]) -> Result<(), Error> {
    if dest.is_empty() {
        return Ok(());
    }
    imp::fill(dest)
}

#[cfg(windows)]
mod imp {
    use super::Error;

    // Same declaration the standard library uses. raw-dylib means no import
    // library is needed, and the DLL is one std already loads.
    #[link(name = "bcryptprimitives", kind = "raw-dylib")]
    extern "system" {
        fn ProcessPrng(pbdata: *mut u8, cbdata: usize) -> i32;
    }

    pub fn fill(dest: &mut [u8]) -> Result<(), Error> {
        // SAFETY: the pointer and length describe a valid, writable buffer.
        // ProcessPrng is documented never to fail; check anyway.
        let ok = unsafe { ProcessPrng(dest.as_mut_ptr(), dest.len()) };
        if ok != 0 {
            Ok(())
        } else {
            Err(Error::UNEXPECTED)
        }
    }
}

#[cfg(unix)]
mod imp {
    // Non-Windows builds exist for development and tests. The kernel's
    // urandom device is the portable source; std is pulled in only here.
    extern crate std;
    use super::Error;
    use std::fs::File;
    use std::io::Read;

    pub fn fill(dest: &mut [u8]) -> Result<(), Error> {
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(dest))
            .map_err(|_| Error::UNEXPECTED)
    }
}

#[cfg(not(any(windows, unix)))]
compile_error!("getrandom shim: unsupported target");
