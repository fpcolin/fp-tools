//! Command-line front end, for scripts and for testing. It reads and writes
//! the same per-user settings as the window.
//!
//! The exe is a Windows GUI program, so `cmd` does not wait for it; use
//! `start /wait FPTools.exe ...` (or `Start-Process -Wait` in PowerShell) when
//! the exit code matters.

use std::path::Path;
use std::process::ExitCode;

use crate::APP;
use crate::common::policy::Policy;
use crate::common::{shell, updater};
use crate::config::{self, DEFAULT_LOGO};
use crate::converter::{self, Mode};
use crate::generator::{self, GenerateError};
use crate::vcard::Contact;

const USAGE: &str = "\
Usage:
  FPTools                                  open the window

QR codes:
  FPTools url <web address>
  FPTools vcard --first <name> --last <name> [--email <address>]
                [--mobile <number>] [--work <number>] [--company <name>]
                [--title <title>] [--website <web address>]

VendorCafe converter:
  FPTools select <file.csv>                choose the exported CSV
  FPTools convert po|wo [<file.csv>]       po: the customer uses a PO number
                                           wo: the customer uses a work order number

Settings:
  FPTools settings
  FPTools set folder <folder>              where QR codes are saved
  FPTools set filename <name>              file name for new QR codes
  FPTools set image <file>|default         image in the middle of QR codes
  FPTools set <option> on|off              open-image, open-folder, embed-logo,
                                           round-edges, convert-overwrite,
                                           convert-open-folder

Updates:
  FPTools check-updates
  FPTools update                           download and install the newest version
  FPTools --version";

pub fn run(args: &[String]) -> ExitCode {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h" | "/?"] => {
            println!("{} v{}\n\n{USAGE}", crate::app::DISPLAY_NAME, APP.version);
            ExitCode::SUCCESS
        }
        ["--version" | "-V"] => {
            println!("{}", APP.version);
            ExitCode::SUCCESS
        }
        ["url", url] => generate(generator::url_payload(url)),
        ["vcard", rest @ ..] => match parse_contact(rest) {
            Ok(contact) => generate(generator::vcard_payload(&contact)),
            Err(message) => usage_error(&message),
        },
        ["select", file] => select(file),
        ["convert", mode, rest @ ..] if rest.len() <= 1 => {
            let mode = match mode.to_ascii_lowercase().as_str() {
                "po" => Mode::PurchaseOrder,
                "wo" => Mode::WorkOrder,
                _ => return usage_error("Use po or wo."),
            };
            convert(mode, rest.first().copied())
        }
        ["settings"] => show_settings(),
        ["set", key, value] => set(key, value),
        ["check-updates"] => check_updates(),
        ["update"] => update(),
        _ => usage_error("Unrecognised command."),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("{message}\n\n{USAGE}");
    ExitCode::from(2)
}

fn parse_contact(args: &[&str]) -> Result<Contact, String> {
    let mut contact = Contact::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value."))?.to_string();
        let slot = match *flag {
            "--first" => &mut contact.first_name,
            "--last" => &mut contact.last_name,
            "--email" => &mut contact.email,
            "--mobile" => &mut contact.mobile,
            "--work" => &mut contact.work_phone,
            "--company" => &mut contact.company,
            "--title" => &mut contact.title,
            "--website" => &mut contact.website,
            other => return Err(format!("Unknown option {other}.")),
        };
        *slot = value;
    }
    Ok(contact)
}

fn generate(payload: Result<String, GenerateError>) -> ExitCode {
    let cfg = config::load();
    let qr = &cfg.get().qr;
    match payload.and_then(|data| generator::save(&data, qr)) {
        Ok(path) => {
            println!("Saved {}", path.display());
            generator::reveal(&path, qr);
            ExitCode::SUCCESS
        }
        Err(e) if e.is_permission_denied() => {
            fail(&format!("{e}\n\nChoose a different folder with: FPTools set folder <folder>"))
        }
        Err(e) => fail(&e.to_string()),
    }
}

fn select(file: &str) -> ExitCode {
    let path = match std::path::absolute(file) {
        Ok(p) if p.is_file() => p,
        _ => return fail(&format!("Not an existing file: {file}")),
    };
    let mut cfg = config::load();
    if let Err(e) = cfg.update(|s| s.vc.filepath = path.to_string_lossy().into_owned()) {
        eprintln!("Warning: the choice could not be saved ({e}).");
    }
    println!("Selected file: {}", path.display());
    ExitCode::SUCCESS
}

fn convert(mode: Mode, file: Option<&str>) -> ExitCode {
    let cfg = config::load();
    let vc = &cfg.get().vc;
    let result = match file {
        Some(f) => converter::convert_file(Path::new(f), mode, vc.option_overwrite),
        None => converter::convert_selected(vc, mode),
    };
    match result {
        Ok(outcome) => {
            println!("{}", outcome.summary());
            if vc.option_open_folder {
                let _ = shell::reveal(&outcome.target);
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e.to_string()),
    }
}

fn on_off(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn set(key: &str, value: &str) -> ExitCode {
    let mut cfg = config::load();
    let result = match key {
        "folder" => {
            let folder = match std::path::absolute(value) {
                Ok(p) if p.is_dir() => p,
                _ => return fail(&format!("Not an existing folder: {value}")),
            };
            cfg.update(|s| s.qr.saveto_path = folder.to_string_lossy().into_owned())
        }
        "filename" => {
            let name = value.trim();
            let name = if name.is_empty() { config::DEFAULT_FILE_NAME } else { name };
            cfg.update(|s| s.qr.saveto_name = name.to_string())
        }
        "image" if value.eq_ignore_ascii_case("default") => cfg.update(|s| s.qr.embed_image = DEFAULT_LOGO.to_string()),
        "image" => {
            let path = match std::path::absolute(value) {
                Ok(p) => p,
                Err(e) => return fail(&e.to_string()),
            };
            // Check it now rather than at the next QR code.
            if let Err(e) = generator::load_image_file(&path) {
                return fail(&e.to_string());
            }
            cfg.update(|s| s.qr.embed_image = path.to_string_lossy().into_owned())
        }
        "open-image" | "open-folder" | "embed-logo" | "round-edges" | "convert-overwrite" | "convert-open-folder" => {
            let Some(on) = on_off(value) else { return usage_error("Use on or off.") };
            cfg.update(|s| match key {
                "open-image" => s.qr.option_open_image = on,
                "open-folder" => s.qr.option_open_folder = on,
                "embed-logo" => s.qr.option_embed_logo = on,
                "round-edges" => s.qr.option_round_edges = on,
                "convert-overwrite" => s.vc.option_overwrite = on,
                _ => s.vc.option_open_folder = on,
            })
        }
        _ => return usage_error(&format!("Unknown setting {key}.")),
    };
    if let Err(e) = result {
        eprintln!("Warning: the setting applies to this run only; it could not be saved ({e}).");
    }
    show_settings()
}

fn show_settings() -> ExitCode {
    let cfg = config::load();
    let (qr, vc) = (&cfg.get().qr, &cfg.get().vc);
    let yn = |b: bool| if b { "on" } else { "off" };
    let file = cfg.path().map_or_else(|| "(not saved)".to_string(), |p| p.display().to_string());
    println!("Settings file:        {file}");
    println!();
    println!("QR codes");
    println!("  Save folder:        {}", qr.saveto_path);
    println!("  File name:          {}.png", generator::safe_file_stem(&qr.saveto_name));
    println!(
        "  Image:              {}",
        if qr.embed_image == DEFAULT_LOGO { "Flooring Partners logo" } else { &qr.embed_image }
    );
    println!("  open-image:         {}", yn(qr.option_open_image));
    println!("  open-folder:        {}", yn(qr.option_open_folder));
    println!("  embed-logo:         {}", yn(qr.option_embed_logo));
    println!("  round-edges:        {}", yn(qr.option_round_edges));
    println!();
    println!("VendorCafe converter");
    println!("  Selected file:      {}", if vc.filepath.is_empty() { "none selected" } else { &vc.filepath });
    println!("  convert-overwrite:  {}", yn(vc.option_overwrite));
    println!("  convert-open-folder: {}", yn(vc.option_open_folder));
    ExitCode::SUCCESS
}

fn check_updates() -> ExitCode {
    let policy = Policy::load();
    match updater::check(&APP, &policy) {
        Ok(Some(release)) => {
            println!("Version {} is available. You have {}.", release.version, APP.version);
            if !release.notes.is_empty() {
                println!("\n{}", release.notes);
            }
            println!("\n{}\nRun \"FPTools update\" to install it.", updater::install_hint(&policy));
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("You are running the latest version ({}).", APP.version);
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e.to_string()),
    }
}

fn update() -> ExitCode {
    let policy = Policy::load();
    let release = match updater::check(&APP, &policy) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("You are running the latest version ({}).", APP.version);
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(&e.to_string()),
    };
    println!("Downloading version {}...", release.version);
    let mut last = u64::MAX;
    let downloaded = updater::download(&APP, &policy, &release, |got, total| {
        if let Some(total) = total.filter(|&t| t > 0) {
            let pct = got * 100 / total;
            if pct / 10 != last / 10 {
                eprint!("{pct}% ");
                last = pct;
            }
        }
        true
    });
    eprintln!();
    let downloaded = match downloaded {
        Ok(d) => d,
        Err(e) => return fail(&e.to_string()),
    };
    println!("{}", updater::install_hint(&policy));
    // The installer file stays locked until Setup has finished (or has closed
    // this program to replace it). /NORELAUNCH: a script, not a person,
    // started this, so the window is not opened afterwards.
    match downloaded.launch_without_relaunch(&policy) {
        Ok(running) => {
            println!("Installer started; waiting for it to finish...");
            #[cfg(windows)]
            let ok = crate::gui::wait_for_installer(running);
            #[cfg(not(windows))]
            let ok = running.wait();
            if ok {
                println!("Update installed.");
                ExitCode::SUCCESS
            } else {
                fail("The update was not installed (it failed, or administrator approval was declined).")
            }
        }
        Err(e) => fail(&e.to_string()),
    }
}
