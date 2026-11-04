//! Embeds the icon, the Windows version resource (File Properties > Details)
//! and the application manifest into the exe. No build dependencies: it
//! drives the resource compiler that comes with the toolchain (rc.exe from the
//! Windows SDK for MSVC builds, windres for GNU builds).
//!
//! Set FP_REQUIRE_RESOURCES=1 (the release workflow does) to make a missing
//! resource compiler an error instead of a warning.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const COMPANY: &str = "Flooring Partners";
const PRODUCT: &str = "Flooring Partners App";
const DESCRIPTION: &str = "Flooring Partners App";
const EXE_NAME: &str = "FPTools";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/fp.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-env-changed=FP_REQUIRE_RESOURCES");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=WINDRES");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = embed_resources() {
        if env::var_os("FP_REQUIRE_RESOURCES").is_some() {
            panic!("could not embed Windows resources: {e}");
        }
        println!("cargo:warning=Windows resources (icon, version info, manifest) not embedded: {e}");
    }
}

fn rc_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\").replace('"', "\"\"")
}

fn rc_text(s: &str) -> String {
    s.replace('"', "\"\"")
}

fn embed_resources() -> Result<(), String> {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let out = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let v = |name: &str| env::var(name).unwrap_or_else(|_| "0".into());
    let (major, minor, patch) =
        (v("CARGO_PKG_VERSION_MAJOR"), v("CARGO_PKG_VERSION_MINOR"), v("CARGO_PKG_VERSION_PATCH"));
    let version = format!("{major}.{minor}.{patch}.0");

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{company}"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{exe}"
      VALUE "LegalCopyright", "Copyright (c) {company}"
      VALUE "OriginalFilename", "{exe}.exe"
      VALUE "ProductName", "{product}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = rc_path(&root.join("assets").join("fp.ico")),
        manifest = rc_path(&root.join("assets").join("app.manifest")),
        company = rc_text(COMPANY),
        description = rc_text(DESCRIPTION),
        product = rc_text(PRODUCT),
        exe = rc_text(EXE_NAME),
    );
    let rc_file = out.join("app.rc");
    fs::write(&rc_file, rc).map_err(|e| e.to_string())?;

    let compiled = if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let res = out.join("app.res");
        let rc_exe = find_rc_exe().ok_or("rc.exe not found (install the Windows SDK, or set RC)")?;
        run(Command::new(rc_exe).arg("/nologo").arg("/fo").arg(&res).arg(&rc_file))?;
        res
    } else {
        let obj = out.join("app_res.o");
        let windres = env::var_os("WINDRES").map(PathBuf::from).unwrap_or_else(|| {
            let target = env::var("TARGET").unwrap_or_default();
            let prefixed = if target.starts_with("x86_64") { "x86_64-w64-mingw32-windres" } else { "windres" };
            if Command::new(prefixed).arg("--version").output().is_ok() {
                PathBuf::from(prefixed)
            } else {
                PathBuf::from("windres")
            }
        });
        run(Command::new(windres).arg("-i").arg(&rc_file).arg("-O").arg("coff").arg("-o").arg(&obj))?;
        obj
    };
    // Bins only: the library and tests do not need an icon.
    println!("cargo:rustc-link-arg-bins={}", compiled.display());
    Ok(())
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// rc.exe from the newest installed Windows 10/11 SDK.
fn find_rc_exe() -> Option<PathBuf> {
    if let Some(rc) = env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let arch = match env::var("HOST").unwrap_or_default().split('-').next() {
        Some("aarch64") => "arm64",
        Some("i686") => "x86",
        _ => "x64",
    };
    let roots = [env::var_os("ProgramFiles(x86)"), env::var_os("ProgramFiles")];
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for root in roots.into_iter().flatten() {
        let bin = PathBuf::from(root).join("Windows Kits").join("10").join("bin");
        let Ok(entries) = fs::read_dir(&bin) else { continue };
        for entry in entries.flatten() {
            let candidate = entry.path().join(arch).join("rc.exe");
            if !candidate.is_file() {
                continue;
            }
            let key: Vec<u32> =
                entry.file_name().to_string_lossy().split('.').map(|p| p.parse().unwrap_or(0)).collect();
            if best.as_ref().is_none_or(|(k, _)| key > *k) {
                best = Some((key, candidate));
            }
        }
    }
    best.map(|(_, path)| path)
}
