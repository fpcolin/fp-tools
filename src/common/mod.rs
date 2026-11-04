//! Code shared by the Flooring Partners desktop tools.
//!
//! Design rule: nothing here loads a Windows DLL beyond what the Rust
//! standard library already imports. Everything goes through the standard
//! library or pure-Rust code; the one exception, registry reads in
//! [`winreg`], uses an API set that resolves to kernelbase.dll, which every
//! process has loaded anyway. That keeps application-control policies
//! (ThreatLocker and similar) simple. The window (crate::gui) is the only
//! code that adds DLLs: user32, gdi32 and comdlg32.

pub mod config_store;
pub mod fsutil;
pub mod http;
pub mod json;
pub mod paths;
pub mod policy;
pub mod shell;
pub mod updater;
#[cfg(windows)]
pub mod winreg;

/// Identity and update endpoints of one application.
#[derive(Debug, Clone, Copy)]
pub struct AppInfo {
    /// Company name, used in the settings path and window titles.
    pub vendor: &'static str,
    /// Product name, used in the settings path and window titles.
    pub name: &'static str,
    /// Running version, normally `env!("CARGO_PKG_VERSION")`.
    pub version: &'static str,
    /// Bumped only when the shape of config.json changes.
    pub config_schema: u32,
    /// HTTP User-Agent for the update check.
    pub user_agent: &'static str,
    /// Where the update manifest (version.json) is published.
    pub manifest_url: &'static str,
    /// Every installer URL in the manifest must start with this. Pins
    /// downloads to this project's own releases.
    pub download_prefix: &'static str,
    /// File-name stem for downloaded installers, e.g. "fp_tools_setup".
    pub installer_stem: &'static str,
}

impl AppInfo {
    /// "Flooring Partners QR Code Generator v3.0.0"
    pub fn title(&self) -> String {
        format!("{} {} v{}", self.vendor, self.name, self.version)
    }
}
