//! The operations behind the window's buttons: build the text to encode,
//! render it, save it as a PNG, and show the result.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::common::{fsutil, paths, shell};
use crate::config::{DEFAULT_FILE_NAME, DEFAULT_LOGO, QrSettings as Settings};
use crate::image::{self, Image, Limits, png};
use crate::qr::{EcLevel, QrCode};
use crate::render::{self, Style};
use crate::vcard::{Contact, VCardError};

/// The Flooring Partners logo, compiled into the program.
const BUILT_IN_LOGO: &[u8] = include_bytes!("../assets/fp_logo.png");
/// Largest logo file accepted (decoded size is limited separately).
const MAX_LOGO_FILE: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum GenerateError {
    /// The URL box is empty.
    NothingToEncode,
    VCard(VCardError),
    /// More text than a QR code can hold.
    TooLong,
    /// The embedded image could not be used.
    Logo {
        path: PathBuf,
        reason: String,
    },
    /// Writing the PNG failed. `permission` is true for access-denied errors,
    /// where the window should offer to pick another folder.
    CannotSave {
        folder: PathBuf,
        error: io::Error,
        permission: bool,
    },
}

impl GenerateError {
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, GenerateError::CannotSave { permission: true, .. })
    }
}

impl fmt::Display for GenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenerateError::NothingToEncode => f.write_str("Enter a web address first."),
            GenerateError::VCard(e) => e.fmt(f),
            GenerateError::TooLong => f.write_str("There is too much text to fit in a QR code."),
            GenerateError::Logo { path, reason } => {
                write!(f, "Could not use the embedded image:\n{}\n\n{reason}", path.display())
            }
            GenerateError::CannotSave { folder, permission: true, .. } => write!(
                f,
                "Windows would not let the program save to:\n{}\n\nThis is usually caused by antivirus \
                 ransomware protection (\"Controlled folder access\"), by the folder being managed by \
                 OneDrive, or by the file already being open in another program.",
                folder.display()
            ),
            GenerateError::CannotSave { folder, error, .. } => {
                write!(f, "Could not save the QR code to:\n{}\n\n{error}", folder.display())
            }
        }
    }
}

impl std::error::Error for GenerateError {}

/// Text for the URL tab: trimmed, and not empty.
pub fn url_payload(text: &str) -> Result<String, GenerateError> {
    let url = text.trim();
    if url.is_empty() { Err(GenerateError::NothingToEncode) } else { Ok(url.to_string()) }
}

/// Text for the vCard tab.
pub fn vcard_payload(contact: &Contact) -> Result<String, GenerateError> {
    contact.to_vcard().map_err(GenerateError::VCard)
}

/// Make a configured file name safe to use on Windows. Beyond the characters
/// the Python build removed, this drops control characters, trailing dots and
/// spaces (which Windows silently strips), and avoids device names such as
/// CON or NUL that would send the image nowhere.
pub fn safe_file_stem(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c)).collect();
    let mut stem = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if stem.chars().count() > 150 {
        stem = stem.chars().take(150).collect::<String>().trim_end_matches(['.', ' ']).to_string();
    }
    if stem.is_empty() {
        return DEFAULT_FILE_NAME.to_string();
    }
    let base = stem.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit())
        || ((base.starts_with("COM") || base.starts_with("LPT")) && ["¹", "²", "³"].iter().any(|s| base[3..] == **s));
    if reserved {
        stem.insert(0, '_');
    }
    stem
}

/// Decode an image file for embedding, with size limits.
pub fn load_image_file(path: &Path) -> Result<Image, GenerateError> {
    let fail = |reason: String| GenerateError::Logo { path: path.to_path_buf(), reason };
    let bytes = fsutil::read_capped(path, MAX_LOGO_FILE).map_err(|e| fail(e.to_string()))?;
    decode_guarded(&bytes).map_err(fail)
}

/// Decoders are written not to panic, but a logo is user input: contain any
/// bug to an error message instead of a crash.
fn decode_guarded(bytes: &[u8]) -> Result<Image, String> {
    match std::panic::catch_unwind(|| image::decode(bytes, &Limits::default())) {
        Ok(Ok(img)) => Ok(img),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("the image could not be read".to_string()),
    }
}

fn built_in_logo() -> Result<&'static Image, GenerateError> {
    static LOGO: OnceLock<Result<Image, String>> = OnceLock::new();
    LOGO.get_or_init(|| decode_guarded(BUILT_IN_LOGO))
        .as_ref()
        .map_err(|reason| GenerateError::Logo { path: PathBuf::from(DEFAULT_LOGO), reason: reason.clone() })
}

/// The image `embed_image` names (`DEFAULT_LOGO` or a file). A custom image
/// that has since been deleted gives `None`, as before, rather than an error.
pub fn logo_for(embed_image: &str) -> Result<Option<std::borrow::Cow<'static, Image>>, GenerateError> {
    if embed_image == DEFAULT_LOGO {
        return built_in_logo().map(|img| Some(std::borrow::Cow::Borrowed(img)));
    }
    let path = Path::new(embed_image);
    if !path.is_file() {
        return Ok(None);
    }
    load_image_file(path).map(|img| Some(std::borrow::Cow::Owned(img)))
}

/// The logo to embed under the current settings, if any.
pub fn current_logo(settings: &Settings) -> Result<Option<std::borrow::Cow<'static, Image>>, GenerateError> {
    if !settings.option_embed_logo {
        return Ok(None);
    }
    logo_for(&settings.embed_image)
}

/// Render `data` with the current settings. Also suitable for a preview.
pub fn render(data: &str, settings: &Settings) -> Result<Image, GenerateError> {
    let logo = current_logo(settings)?;
    render_with(data, settings.option_round_edges, logo.as_deref())
}

/// Render with an already loaded logo (the window keeps it decoded for the
/// live preview instead of reading the file at every keystroke).
pub fn render_with(data: &str, round_edges: bool, logo: Option<&Image>) -> Result<Image, GenerateError> {
    // Level H as before: it is required for an embedded logo, and keeps the
    // code readable when printed small or scuffed.
    let qr = QrCode::encode_text(data, EcLevel::H).map_err(|_| GenerateError::TooLong)?;
    Ok(render::render(&qr, &Style { round_edges, logo }))
}

/// Render and save as `<folder>\<name>.png` (or `<name> (1).png`...). Returns
/// the path written.
pub fn save(data: &str, settings: &Settings) -> Result<PathBuf, GenerateError> {
    let image = render(data, settings)?;
    let bytes = png::encode(&image);

    let folder =
        if settings.saveto_path.trim().is_empty() { paths::downloads() } else { PathBuf::from(&settings.saveto_path) };
    let fail = |error: io::Error| GenerateError::CannotSave {
        folder: folder.clone(),
        permission: error.kind() == io::ErrorKind::PermissionDenied,
        error,
    };
    std::fs::create_dir_all(&folder).map_err(fail)?;
    let (path, mut file) =
        fsutil::create_unique(&folder, &safe_file_stem(&settings.saveto_name), ".png").map_err(fail)?;
    if let Err(e) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(fail(e));
    }
    Ok(path)
}

/// Open the saved image and/or its folder, as the options say. Failures are
/// ignored: the file is saved either way.
pub fn reveal(path: &Path, settings: &Settings) {
    if settings.option_open_image {
        let _ = shell::open(path);
    }
    if settings.option_open_folder {
        let _ = shell::reveal(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(safe_file_stem("  qr:code?  "), "qrcode");
        assert_eq!(safe_file_stem("..."), "qr_code");
        assert_eq!(safe_file_stem("report. "), "report");
        assert_eq!(safe_file_stem("con"), "_con");
        assert_eq!(safe_file_stem("NUL.backup"), "_NUL.backup");
        assert_eq!(safe_file_stem("com7"), "_com7");
        assert_eq!(safe_file_stem("computer"), "computer");
        assert_eq!(safe_file_stem("a\u{0}b\tc"), "abc");
        assert_eq!(safe_file_stem(&"x".repeat(400)).len(), 150);
        assert_eq!(safe_file_stem("..\\..\\Windows\\evil"), "....Windowsevil");
    }

    #[test]
    fn built_in_logo_decodes() {
        let logo = built_in_logo().expect("bundled logo must decode");
        assert_eq!((logo.width, logo.height), (680, 680));
    }
}
