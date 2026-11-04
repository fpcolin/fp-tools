//! Per-user settings in `%LOCALAPPDATA%\<vendor>\<app>\config.json`.
//!
//! Same file, same format and same reset rules as the Python builds, so
//! existing settings carry over:
//! * The file is reset only when its `schema` number is not ours, never just
//!   because the program version changed.
//! * Unknown keys are dropped; a key with the wrong type falls back to its
//!   default instead of breaking the program later.
//! * Writes are atomic (temp file + rename), so a crash or a second copy of
//!   the program can never leave a half-written file.
//! * If the profile is read-only the program carries on with in-memory values.
//!
//! Each user has their own file, so users on a shared machine never see or
//! overwrite each other's settings.

use std::io;
use std::path::{Path, PathBuf};

use super::json::{self, Value};
use super::{AppInfo, fsutil, paths};

const FILE_NAME: &str = "config.json";
const LEGACY_FILE_NAME: &str = "config.yml";
const MAX_SIZE: u64 = 1024 * 1024;

/// An application's settings record.
pub trait Settings: Clone + PartialEq + Default {
    /// The fields in file order, excluding `schema` and `version`.
    fn to_fields(&self) -> Vec<(&'static str, Value)>;
    /// Copy every recognised, correctly typed field out of a JSON object.
    fn merge_from(&mut self, stored: &Value);
}

/// Copy `key` into `slot` if it is a string.
pub fn take_string(stored: &Value, key: &str, slot: &mut String) {
    if let Some(s) = stored.get(key).and_then(Value::as_str) {
        *slot = s.to_string();
    }
}

/// Copy `key` into `slot` if it is a boolean.
pub fn take_bool(stored: &Value, key: &str, slot: &mut bool) {
    if let Some(b) = stored.get(key).and_then(Value::as_bool) {
        *slot = b;
    }
}

pub struct Config<S: Settings> {
    app: AppInfo,
    path: Option<PathBuf>,
    settings: S,
}

impl<S: Settings> Config<S> {
    /// Load the current user's settings.
    pub fn load(app: &AppInfo) -> Self {
        let path = paths::settings_dir(app.vendor, app.name).map(|d| d.join(FILE_NAME));
        Self::load_from(app, path)
    }

    /// Load from an explicit path (`None` = keep everything in memory).
    pub fn load_from(app: &AppInfo, path: Option<PathBuf>) -> Self {
        let mut config = Config { app: *app, path, settings: S::default() };
        match config.read_stored() {
            Some(stored) => {
                config.settings.merge_from(&stored);
                // Record which build last wrote the file.
                if stored.get("version").and_then(Value::as_str) != Some(app.version) {
                    let _ = config.save();
                }
            }
            None => {
                let _ = config.save();
            }
        }
        if let Some(path) = &config.path {
            // Clean up the file from versions that used YAML.
            let _ = std::fs::remove_file(path.with_file_name(LEGACY_FILE_NAME));
        }
        config
    }

    pub fn get(&self) -> &S {
        &self.settings
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Change settings and save if anything actually changed.
    ///
    /// Before saving, the file is re-read and only the fields changed here are
    /// applied to it, so a change made meanwhile by another open copy of the
    /// program (same user) is kept rather than overwritten.
    pub fn update(&mut self, change: impl FnOnce(&mut S)) -> io::Result<()> {
        let before = self.settings.clone();
        change(&mut self.settings);
        if before == self.settings {
            return Ok(());
        }
        if let Some(stored) = self.read_stored() {
            let old = before.to_fields();
            let changed: Vec<(String, Value)> = self
                .settings
                .to_fields()
                .into_iter()
                .zip(old)
                .filter(|((_, new), (_, old))| new != old)
                .map(|((key, new), _)| (key.to_string(), new))
                .collect();
            let mut merged = S::default();
            merged.merge_from(&stored);
            merged.merge_from(&Value::Object(changed));
            self.settings = merged;
        }
        self.save()
    }

    /// The stored document, if it exists and has our schema.
    fn read_stored(&self) -> Option<Value> {
        let path = self.path.as_ref()?;
        let bytes = fsutil::read_capped(path, MAX_SIZE).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let stored = json::parse(&text).ok()?;
        let schema = stored.get("schema").and_then(Value::as_f64)?;
        (schema == f64::from(self.app.config_schema)).then_some(stored)
    }

    fn document(&self) -> Value {
        let mut members = vec![
            ("schema".to_string(), Value::Number(f64::from(self.app.config_schema))),
            ("version".to_string(), Value::String(self.app.version.to_string())),
        ];
        members.extend(self.settings.to_fields().into_iter().map(|(k, v)| (k.to_string(), v)));
        Value::Object(members)
    }

    /// Write the settings file. Errors mean a locked-down profile; callers
    /// may ignore them and keep working from memory.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        fsutil::write_atomic(path, json::to_string_pretty(&self.document()).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct Demo {
        name: String,
        flag: bool,
        other: bool,
    }
    impl Default for Demo {
        fn default() -> Self {
            Demo { name: "qr_code".into(), flag: true, other: false }
        }
    }
    impl Settings for Demo {
        fn to_fields(&self) -> Vec<(&'static str, Value)> {
            vec![
                ("name", Value::String(self.name.clone())),
                ("flag", Value::Bool(self.flag)),
                ("other", Value::Bool(self.other)),
            ]
        }
        fn merge_from(&mut self, s: &Value) {
            take_string(s, "name", &mut self.name);
            take_bool(s, "flag", &mut self.flag);
            take_bool(s, "other", &mut self.other);
        }
    }

    const APP: AppInfo = AppInfo {
        vendor: "V",
        name: "N",
        version: "3.0.0",
        config_schema: 1,
        user_agent: "",
        manifest_url: "",
        download_prefix: "",
        installer_stem: "",
    };

    fn temp_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cfg-test-{}", fsutil::random_token()));
        dir.join("sub").join(FILE_NAME)
    }

    #[test]
    fn migrates_python_file_and_keeps_values() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path.with_file_name("config.yml"), "old").unwrap();
        // As written by the Python build: other version, an unknown key, a
        // wrongly typed key.
        std::fs::write(&path, r#"{"schema": 1, "version": "2.1.2", "name": "Zoë", "flag": "yes", "junk": 1}"#).unwrap();
        let cfg: Config<Demo> = Config::load_from(&APP, Some(path.clone()));
        assert_eq!(cfg.get().name, "Zoë");
        assert!(cfg.get().flag, "wrongly typed value must fall back to the default");
        assert!(!path.with_file_name("config.yml").exists());
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written,
            "{\n  \"schema\": 1,\n  \"version\": \"3.0.0\",\n  \"name\": \"Zo\\u00eb\",\n  \"flag\": true,\n  \"other\": false\n}"
        );
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn resets_on_schema_change_and_merges_concurrent_edits() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"schema": 2, "name": "x"}"#).unwrap();
        let mut a: Config<Demo> = Config::load_from(&APP, Some(path.clone()));
        assert_eq!(a.get(), &Demo::default());

        // A second copy of the program changes another field meanwhile.
        let mut b: Config<Demo> = Config::load_from(&APP, Some(path.clone()));
        b.update(|s| s.other = true).unwrap();
        a.update(|s| s.name = "invoice".into()).unwrap();

        let c: Config<Demo> = Config::load_from(&APP, Some(path.clone()));
        assert_eq!(c.get(), &Demo { name: "invoice".into(), flag: true, other: true });
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn unwritable_location_keeps_working_in_memory() {
        let mut cfg: Config<Demo> = Config::load_from(&APP, None);
        cfg.update(|s| s.flag = false).unwrap();
        assert!(!cfg.get().flag);
    }
}
