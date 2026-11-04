//! Identity of this application. To fork it, change these values and replace
//! the files in assets/ (see LICENSE.txt about the Flooring Partners marks).

use crate::common::AppInfo;

pub const APP: AppInfo = AppInfo {
    vendor: "Flooring Partners",
    // Internal name, used for the settings and update folders
    // (%LOCALAPPDATA%\Flooring Partners\Tools). Kept from when the program was
    // called "Flooring Partners Tools" so existing settings are found; the
    // name people see is DISPLAY_NAME.
    name: "Tools",
    // Cargo.toml is the single source of truth for the version.
    version: env!("CARGO_PKG_VERSION"),
    // Bumped only when the shape of config.json changes, never for an
    // ordinary release, so settings survive upgrades.
    config_schema: 1,
    user_agent: concat!("FPTools-Updater/", env!("CARGO_PKG_VERSION")),
    // /releases/latest/download/ is a permanent redirect to the newest
    // release's asset. Do not switch to api.github.com: its 60-requests-per-
    // hour unauthenticated limit is shared by a whole office behind one NAT.
    manifest_url: "https://github.com/fpcolin/fp-tools/releases/latest/download/version.json",
    download_prefix: "https://github.com/fpcolin/fp-tools/releases/download/",
    installer_stem: "fp_tools_setup",
};

/// Window title and the name shown in Apps & features.
pub const DISPLAY_NAME: &str = "Flooring Partners App";
