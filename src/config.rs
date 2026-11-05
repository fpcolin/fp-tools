//! The user's saved settings for both tools, in one per-user file:
//! `%LOCALAPPDATA%\Flooring Partners\Tools\config.json`.
//!
//! The first time a user runs this program, the settings of the separate QR
//! Code Generator and VendorCafe CSV Converter apps (Python 2.x or Rust 3.x,
//! same files) are copied in, so nobody has to set up their folders again.

use std::path::PathBuf;

use crate::common::config_store::{self, take_bool, take_string};
use crate::common::json::{self, Value};
use crate::common::{fsutil, paths};

/// Value of `qr_embed_image` meaning "the built-in Flooring Partners logo".
pub const DEFAULT_LOGO: &str = "default-logo";
/// Used when the configured file name is blank or sanitises to nothing.
pub const DEFAULT_FILE_NAME: &str = "qr_code";

/// QR code settings. Defaults match the old QR Code Generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrSettings {
    /// Folder QR codes are saved to.
    pub saveto_path: String,
    /// File name (without .png) for new QR codes.
    pub saveto_name: String,
    pub option_open_image: bool,
    pub option_open_folder: bool,
    pub option_embed_logo: bool,
    pub option_round_edges: bool,
    /// `DEFAULT_LOGO` or the path of the user's own image.
    pub embed_image: String,
}

impl Default for QrSettings {
    fn default() -> Self {
        QrSettings {
            saveto_path: paths::downloads().to_string_lossy().into_owned(),
            saveto_name: DEFAULT_FILE_NAME.to_string(),
            option_open_image: true,
            option_open_folder: false,
            option_embed_logo: true,
            option_round_edges: false,
            embed_image: DEFAULT_LOGO.to_string(),
        }
    }
}

/// VendorCafe converter settings. Defaults match the old converter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VcSettings {
    /// The exported CSV chosen with "Change file". Empty until chosen.
    pub filepath: String,
    /// Replace the export (true) or write "<name> converted.csv" beside it.
    pub option_overwrite: bool,
    /// Show the converted file in its folder afterwards.
    pub option_open_folder: bool,
}

impl Default for VcSettings {
    fn default() -> Self {
        VcSettings { filepath: String::new(), option_overwrite: true, option_open_folder: false }
    }
}

/// Which page the window opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Qr,
    Converter,
}

impl Page {
    fn key(self) -> &'static str {
        match self {
            Page::Qr => "qr",
            Page::Converter => "vc",
        }
    }
}

/// What the QR page encodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QrKind {
    Url,
    Contact,
}

impl QrKind {
    fn key(self) -> &'static str {
        match self {
            QrKind::Url => "url",
            QrKind::Contact => "contact",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub qr: QrSettings,
    pub vc: VcSettings,
    pub last_page: Page,
    pub qr_kind: QrKind,
    /// The old separate apps have been removed automatically (or, before
    /// 3.0.3, the user was asked about it), so it is not done again at every
    /// start. Stored under its original key, "old_apps_prompted".
    pub old_apps_handled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            qr: QrSettings::default(),
            vc: VcSettings::default(),
            last_page: Page::Qr,
            qr_kind: QrKind::Url,
            old_apps_handled: false,
        }
    }
}

impl config_store::Settings for Settings {
    fn to_fields(&self) -> Vec<(&'static str, Value)> {
        let s = |v: &str| Value::String(v.to_string());
        vec![
            ("last_page", s(self.last_page.key())),
            ("qr_kind", s(self.qr_kind.key())),
            ("qr_saveto_path", s(&self.qr.saveto_path)),
            ("qr_saveto_name", s(&self.qr.saveto_name)),
            ("qr_open_image", Value::Bool(self.qr.option_open_image)),
            ("qr_open_folder", Value::Bool(self.qr.option_open_folder)),
            ("qr_embed_logo", Value::Bool(self.qr.option_embed_logo)),
            ("qr_round_edges", Value::Bool(self.qr.option_round_edges)),
            ("qr_embed_image", s(&self.qr.embed_image)),
            ("vc_filepath", s(&self.vc.filepath)),
            ("vc_overwrite", Value::Bool(self.vc.option_overwrite)),
            ("vc_open_folder", Value::Bool(self.vc.option_open_folder)),
            ("old_apps_prompted", Value::Bool(self.old_apps_handled)),
        ]
    }

    fn merge_from(&mut self, stored: &Value) {
        match stored.get("last_page").and_then(Value::as_str) {
            Some("qr") => self.last_page = Page::Qr,
            Some("vc") => self.last_page = Page::Converter,
            _ => {}
        }
        match stored.get("qr_kind").and_then(Value::as_str) {
            Some("url") => self.qr_kind = QrKind::Url,
            Some("contact") => self.qr_kind = QrKind::Contact,
            _ => {}
        }
        let qr = &mut self.qr;
        take_string(stored, "qr_saveto_path", &mut qr.saveto_path);
        take_string(stored, "qr_saveto_name", &mut qr.saveto_name);
        take_bool(stored, "qr_open_image", &mut qr.option_open_image);
        take_bool(stored, "qr_open_folder", &mut qr.option_open_folder);
        take_bool(stored, "qr_embed_logo", &mut qr.option_embed_logo);
        take_bool(stored, "qr_round_edges", &mut qr.option_round_edges);
        take_string(stored, "qr_embed_image", &mut qr.embed_image);
        let vc = &mut self.vc;
        take_string(stored, "vc_filepath", &mut vc.filepath);
        take_bool(stored, "vc_overwrite", &mut vc.option_overwrite);
        take_bool(stored, "vc_open_folder", &mut vc.option_open_folder);
        take_bool(stored, "old_apps_prompted", &mut self.old_apps_handled);
    }
}

pub type Config = config_store::Config<Settings>;

/// Product names of the separate apps this program replaces. Their settings
/// folders are `%LOCALAPPDATA%\Flooring Partners\<name>`.
pub const OLD_QR_APP: &str = "QR Code Generator";
pub const OLD_VC_APP: &str = "VendorCafe CSV Converter";

/// Load the current user's settings, copying them from the old separate apps
/// the first time.
pub fn load() -> Config {
    let path = paths::settings_dir(crate::APP.vendor, crate::APP.name).map(|d| d.join("config.json"));
    let first_run = path.as_ref().is_some_and(|p| !p.exists());
    let mut config = Config::load_from(&crate::APP, path);
    if first_run {
        let old_qr = old_settings_file(OLD_QR_APP).and_then(|p| read_old(&p));
        let old_vc = old_settings_file(OLD_VC_APP).and_then(|p| read_old(&p));
        if old_qr.is_some() || old_vc.is_some() {
            let _ = config.update(|s| import_old(s, old_qr.as_ref(), old_vc.as_ref()));
        }
    }
    config
}

fn old_settings_file(app_name: &str) -> Option<PathBuf> {
    paths::settings_dir(crate::APP.vendor, app_name).map(|d| d.join("config.json"))
}

/// An old app's config.json, if it exists and has the schema those apps used.
fn read_old(path: &std::path::Path) -> Option<Value> {
    let bytes = fsutil::read_capped(path, 1024 * 1024).ok()?;
    let stored = json::parse(&String::from_utf8(bytes).ok()?).ok()?;
    (stored.get("schema").and_then(Value::as_f64) == Some(1.0)).then_some(stored)
}

/// Copy the old apps' settings (their own key names) into `settings`.
pub fn import_old(settings: &mut Settings, old_qr: Option<&Value>, old_vc: Option<&Value>) {
    if let Some(v) = old_qr {
        let qr = &mut settings.qr;
        take_string(v, "saveto_path", &mut qr.saveto_path);
        take_string(v, "saveto_name", &mut qr.saveto_name);
        take_bool(v, "option_open_image", &mut qr.option_open_image);
        take_bool(v, "option_open_folder", &mut qr.option_open_folder);
        take_bool(v, "option_embed_logo", &mut qr.option_embed_logo);
        take_bool(v, "option_round_edges", &mut qr.option_round_edges);
        take_string(v, "embed_image", &mut qr.embed_image);
    }
    if let Some(v) = old_vc {
        let vc = &mut settings.vc;
        take_string(v, "filepath", &mut vc.filepath);
        take_bool(v, "option_overwrite", &mut vc.option_overwrite);
        take_bool(v, "option_open_folder", &mut vc.option_open_folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config_store::Settings as _;

    #[test]
    fn imports_both_old_apps() {
        let qr = json::parse(
            r#"{"schema": 1, "version": "2.1.2", "saveto_path": "D:\\QR", "saveto_name": "card",
                "option_open_image": false, "option_open_folder": true, "option_embed_logo": false,
                "option_round_edges": true, "embed_image": "C:\\logo.png"}"#,
        )
        .unwrap();
        let vc = json::parse(
            r#"{"schema": 1, "filepath": "C:\\x.csv", "option_overwrite": false, "option_open_folder": "yes"}"#,
        )
        .unwrap();
        let mut s = Settings::default();
        import_old(&mut s, Some(&qr), Some(&vc));
        assert_eq!(
            s.qr,
            QrSettings {
                saveto_path: "D:\\QR".into(),
                saveto_name: "card".into(),
                option_open_image: false,
                option_open_folder: true,
                option_embed_logo: false,
                option_round_edges: true,
                embed_image: "C:\\logo.png".into(),
            }
        );
        // Wrongly typed values keep the default.
        assert_eq!(
            s.vc,
            VcSettings { filepath: "C:\\x.csv".into(), option_overwrite: false, option_open_folder: false }
        );
    }

    #[test]
    fn round_trips_through_fields() {
        let mut s = Settings { last_page: Page::Converter, qr_kind: QrKind::Contact, ..Default::default() };
        s.qr.saveto_name = "x".into();
        s.vc.option_overwrite = false;
        s.old_apps_handled = true;
        let mut back = Settings::default();
        back.merge_from(&Value::Object(s.to_fields().into_iter().map(|(k, v)| (k.to_string(), v)).collect()));
        assert_eq!(back, s);
    }
}
