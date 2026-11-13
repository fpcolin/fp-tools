//! The sidebar's update card, the update flow, and removal of the old
//! separate apps.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::canvas::RectF;
use super::layout::{Frame, Item, text_height};
use super::theme::{self, ButtonKind, Font, Icon, c};
use super::win32::*;
use super::{Event, Model, PageId, Ui, message, post, relayout_sidebar, ui, widgets, with};
use crate::APP;
use crate::common::policy::{InstallScope, Policy};
use crate::common::updater::{self, Downloaded, Release, UpdateError};

pub enum UpdateState {
    /// Waiting for the automatic check shortly after start-up.
    Pending,
    /// `attempt` counts from 1; the automatic check tries a few times
    /// before saying it failed.
    Checking {
        manual: bool,
        attempt: u8,
    },
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        received: u64,
        total: Option<u64>,
        cancel: Arc<AtomicBool>,
    },
    /// Setup is running; the window is hidden.
    Installing(Release),
    /// The last check failed, and why (shown in Settings, and in the message
    /// for a manual check).
    Failed(String),
    /// Turned off in policy.ini, or policy.ini could not be read.
    Disabled {
        unreadable: bool,
    },
}

impl UpdateState {
    pub fn initial(policy: &Policy) -> UpdateState {
        if policy.load_error.is_some() {
            UpdateState::Disabled { unreadable: true }
        } else if !policy.updates_enabled {
            UpdateState::Disabled { unreadable: false }
        } else {
            UpdateState::Pending
        }
    }

    pub fn should_check_at_start(&self) -> bool {
        matches!(self, UpdateState::Pending)
    }
}

pub enum UpdateEvent {
    Checked { manual: bool, result: Result<Option<Release>, UpdateError> },
    Progress { received: u64, total: Option<u64> },
    Downloaded(Result<Downloaded, UpdateError>),
    InstallerFinished(bool),
}

/// Icon and its colour, status text, button (caption, style) and progress.
type CardContent = (Icon, u32, String, Option<(&'static str, ButtonKind)>, Option<f32>);

/// What the update card shows in the current state.
fn card_content(m: &Model) -> CardContent {
    match &m.update {
        UpdateState::Pending | UpdateState::Checking { .. } => {
            (Icon::Download, c::NAV_MUTED, "Checking for updates…".into(), None, None)
        }
        UpdateState::UpToDate => (
            Icon::CheckCircle,
            c::GOLD,
            "Up to date".into(),
            Some(("Check for updates", ButtonKind::SidebarAction)),
            None,
        ),
        UpdateState::Available(r) => (
            Icon::Download,
            c::GOLD,
            format!("Version {} is available", r.version),
            Some(("Install update", ButtonKind::SidebarGold)),
            None,
        ),
        UpdateState::Downloading { received, total, .. } => {
            let frac = total.filter(|&t| t > 0).map(|t| *received as f32 / t as f32);
            let text = match frac {
                Some(f) => format!("Downloading… {}%", (f * 100.0) as u32),
                None => format!("Downloading… {:.1} MB", *received as f64 / 1_048_576.0),
            };
            (Icon::Download, c::GOLD, text, Some(("Cancel", ButtonKind::SidebarAction)), Some(frac.unwrap_or(0.0)))
        }
        UpdateState::Installing(r) => {
            (Icon::Download, c::GOLD, format!("Installing version {}…", r.version), None, None)
        }
        UpdateState::Failed(_) => (
            Icon::Alert,
            c::NAV_MUTED,
            "Couldn't check for updates".into(),
            Some(("Try again", ButtonKind::SidebarAction)),
            None,
        ),
        UpdateState::Disabled { .. } => {
            (Icon::CheckCircle, c::NAV_MUTED, "Updates are managed by your IT department".into(), None, None)
        }
    }
}

/// Set the update button's caption and style for the current state.
pub fn sync_button() {
    let btn = ui().update_btn;
    let Some(content) = with(|m| card_content(m).3) else { return };
    if let Some((text, kind)) = content {
        theme::set_button_kind(btn, kind, c::NAVY_DARK);
        widgets::set_text(btn, text);
    }
}

pub fn sidebar_layout(ui: &Ui, m: &mut Model, height: f32) -> Frame {
    let mut f = Frame::default();
    f.panel(RectF::new(20.0, 20.0, 40.0, 40.0), c::WHITE, c::WHITE, 9.0);
    if let Some(logo) = super::logo_bitmap(m, crate::config::DEFAULT_LOGO, theme::px(32.0), c::WHITE) {
        f.items.push(Item::Image(RectF::new(24.0, 24.0, 32.0, 32.0), logo));
    }
    f.text(
        RectF::new(72.0, 19.0, 160.0, 22.0),
        "Flooring Partners",
        Font::Brand,
        c::WHITE,
        DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    f.text(RectF::new(72.0, 41.0, 150.0, 18.0), "App", Font::Small, c::NAV_MUTED, DT_SINGLELINE);

    let ys = [88.0, 132.0, 176.0, 242.0, 286.0];
    for (i, y) in ys.into_iter().enumerate() {
        f.place(ui.nav[i], RectF::new(12.0, y, 220.0, 40.0));
    }
    f.items.push(Item::Rule(24.0, 230.0, 196.0, c::SIDEBAR_LINE));

    let (icon, icon_color, status, button, progress) = card_content(m);
    let status_h = text_height(Font::Small, &status, 168.0).max(18.0);
    let mut h = 14.0 + 18.0 + 8.0 + status_h + 14.0;
    if progress.is_some() {
        h += 12.0;
    }
    if button.is_some() {
        h += 34.0 + 2.0;
    }
    let top = (height - 16.0 - h).max(340.0);
    f.panel(RectF::new(12.0, top, 220.0, h), c::NAVY_DARK, c::NAVY_DARK, 10.0);
    f.text(
        RectF::new(26.0, top + 14.0, 180.0, 18.0),
        &format!("Version {}", APP.version),
        Font::SmallBold,
        c::WHITE,
        DT_SINGLELINE,
    );
    let mut y = top + 40.0;
    f.items.push(Item::Icon(RectF::new(26.0, y + 1.0, 16.0, 16.0), icon, icon_color));
    f.text(RectF::new(48.0, y, 170.0, status_h), &status, Font::Small, c::NAV_TEXT, DT_WORDBREAK);
    y += status_h + 14.0;
    if let Some(frac) = progress {
        f.items.push(Item::Progress(RectF::new(26.0, y - 4.0, 192.0, 6.0), frac));
        y += 12.0;
    }
    f.show_if(ui.update_btn, button.is_some(), RectF::new(26.0, y - 2.0, 192.0, 34.0));
    f.height = height;
    f
}

fn set_state(state: UpdateState) {
    with(|m| m.update = state);
    relayout_sidebar();
    super::relayout(PageId::Settings);
}

/// "Check now" on the Settings page.
pub fn check_now() {
    start_check(true);
}

pub fn start_check(manual: bool) {
    let policy_problem = with(|m| match m.update {
        UpdateState::Disabled { unreadable } => Some(unreadable),
        _ => None,
    })
    .flatten();
    if let Some(unreadable) = policy_problem {
        if manual {
            message(
                if unreadable {
                    "Updates are turned off because the settings file from your IT department (policy.ini) could not be read."
                } else {
                    "Updates for this program are managed by your IT department."
                },
                MB_ICONINFORMATION,
            );
        }
        return;
    }
    if with(|m| {
        matches!(m.update, UpdateState::Checking { .. } | UpdateState::Downloading { .. } | UpdateState::Installing(_))
    })
    .unwrap_or(true)
    {
        return;
    }
    begin_check(manual, 1);
}

/// Pauses before the automatic check tries again: a blip in the network,
/// or GitHub being slow for a moment, shouldn't show as a failure.
const RETRY_DELAYS_MS: [u32; 2] = [15_000, 60_000];

fn begin_check(manual: bool, attempt: u8) {
    set_state(UpdateState::Checking { manual, attempt });
    let Some((policy, events)) = with(|m| (m.policy.clone(), m.events.clone())) else { return };
    let main = super::main_handle();
    std::thread::spawn(move || {
        let result = updater::check(&APP, &policy);
        post(&events, main, Event::Update(UpdateEvent::Checked { manual, result }));
    });
}

/// The retry timer fired.
pub fn retry_check() {
    if let Some(Some(attempt)) = with(|m| match m.update {
        UpdateState::Checking { manual: false, attempt } => Some(attempt),
        _ => None,
    }) {
        begin_check(false, attempt + 1);
    }
}

/// The sidebar button: check, install or cancel depending on the state.
pub fn button_clicked() {
    enum Action {
        Check,
        Install(Release),
        Cancel(Arc<AtomicBool>),
        Nothing,
    }
    let action = with(|m| match &m.update {
        UpdateState::UpToDate | UpdateState::Failed(_) => Action::Check,
        UpdateState::Available(r) => Action::Install(r.clone()),
        UpdateState::Downloading { cancel, .. } => Action::Cancel(cancel.clone()),
        _ => Action::Nothing,
    });
    match action {
        Some(Action::Check) => start_check(true),
        Some(Action::Install(r)) => install(r),
        Some(Action::Cancel(flag)) => flag.store(true, Ordering::Relaxed),
        _ => {}
    }
}

fn install(release: Release) {
    let Some(policy) = with(|m| m.policy.clone()) else { return };
    let notes = if release.notes.is_empty() { String::new() } else { format!("\n\n{}", release.notes) };
    let question = format!(
        "Install version {} now? You have {}.{notes}\n\n{}",
        release.version,
        APP.version,
        updater::install_hint(&policy)
    );
    if message(&question, MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    set_state(UpdateState::Downloading { release: release.clone(), received: 0, total: None, cancel: cancel.clone() });
    let Some(events) = with(|m| m.events.clone()) else { return };
    let main = super::main_handle();
    std::thread::spawn(move || {
        let mut last = (0u64, std::time::Instant::now());
        let result = updater::download(&APP, &policy, &release, |received, total| {
            // A few updates per second are plenty for the progress bar.
            if last.1.elapsed().as_millis() >= 150 || Some(received) == total {
                last = (received, std::time::Instant::now());
                post(&events, main, Event::Update(UpdateEvent::Progress { received, total }));
            }
            !cancel.load(Ordering::Relaxed)
        });
        post(&events, main, Event::Update(UpdateEvent::Downloaded(result)));
    });
}

pub fn on_event(event: UpdateEvent) {
    match event {
        UpdateEvent::Checked { manual, result } => match result {
            Ok(Some(release)) => {
                set_state(UpdateState::Available(release.clone()));
                if manual {
                    install(release);
                }
            }
            Ok(None) => {
                set_state(UpdateState::UpToDate);
                if manual {
                    message(&format!("You have the latest version ({}).", APP.version), MB_ICONINFORMATION);
                }
            }
            Err(e) => {
                let attempt = with(|m| match m.update {
                    UpdateState::Checking { attempt, .. } => attempt,
                    _ => 1,
                })
                .unwrap_or(1);
                if !manual && let Some(&delay) = RETRY_DELAYS_MS.get(usize::from(attempt) - 1) {
                    // Still "Checking for updates…" in the sidebar.
                    // SAFETY: live window; one-shot timer.
                    unsafe { SetTimer(ui().main, super::TIMER_UPDATE_RETRY, delay, std::ptr::null()) };
                    return;
                }
                set_state(UpdateState::Failed(e.to_string()));
                if manual {
                    message(&format!("Couldn't check for updates.\n\n{e}"), MB_ICONWARNING);
                }
            }
        },
        UpdateEvent::Progress { received: r, total: t } => {
            let changed = with(|m| match &mut m.update {
                UpdateState::Downloading { received, total, .. } => {
                    *received = r;
                    *total = t;
                    true
                }
                _ => false,
            });
            if changed == Some(true) {
                relayout_sidebar();
            }
        }
        UpdateEvent::Downloaded(result) => {
            let state = with(|m| match &m.update {
                UpdateState::Downloading { release, cancel, .. } => {
                    Some((release.clone(), cancel.load(Ordering::Relaxed)))
                }
                _ => None,
            })
            .flatten();
            let Some((release, cancelled)) = state else {
                if let Ok(downloaded) = result {
                    downloaded.discard();
                }
                return;
            };
            match result {
                // Cancel was pressed after the last of the data arrived.
                Ok(downloaded) if cancelled => {
                    downloaded.discard();
                    set_state(UpdateState::Available(release));
                }
                Ok(downloaded) => launch(release, downloaded),
                Err(UpdateError::Cancelled) => set_state(UpdateState::Available(release)),
                Err(e) => {
                    set_state(UpdateState::Available(release));
                    message(&format!("The update could not be downloaded.\n\n{e}"), MB_ICONWARNING);
                }
            }
        }
        UpdateEvent::InstallerFinished(ok) => {
            if ok {
                // Setup normally closes us itself; if it did not need to,
                // close now so the new version is used next time.
                super::commit_pending_edits();
                // SAFETY: live window.
                unsafe { DestroyWindow(ui().main) };
            } else {
                // SAFETY: live window.
                unsafe {
                    ShowWindow(ui().main, SW_SHOW);
                    SetForegroundWindow(ui().main);
                }
                let release = with(|m| match &m.update {
                    UpdateState::Installing(r) => Some(r.clone()),
                    _ => None,
                })
                .flatten();
                if let Some(r) = release {
                    set_state(UpdateState::Available(r));
                }
                message(
                    "The update was not installed. It may have been cancelled, or administrator approval was declined.",
                    MB_ICONWARNING,
                );
            }
        }
    }
}

fn launch(release: Release, downloaded: Downloaded) {
    let Some((policy, events)) = with(|m| (m.policy.clone(), m.events.clone())) else { return };
    super::commit_pending_edits();
    match downloaded.launch(&policy) {
        Ok(running) => {
            set_state(UpdateState::Installing(release));
            // Get out of Setup's way; it closes the program when it is ready
            // to replace the files.
            // SAFETY: live window.
            unsafe { ShowWindow(ui().main, SW_HIDE) };
            let main = super::main_handle();
            std::thread::spawn(move || {
                let ok = running.wait();
                post(&events, main, Event::Update(UpdateEvent::InstallerFinished(ok)));
            });
        }
        Err(e) => {
            set_state(UpdateState::Available(release));
            message(&format!("The installer could not be started.\n\n{e}"), MB_ICONWARNING);
        }
    }
}

/// Text for "Installed for" on the Settings page.
pub fn scope_text(policy: &Policy) -> &'static str {
    match policy.scope {
        InstallScope::Machine => "Everyone who uses this PC",
        InstallScope::User => "Just you",
        InstallScope::Unknown => "Not installed (running from a folder)",
    }
}

/// Text for "Update checks" on the Settings page.
pub fn checks_text(m: &Model) -> String {
    match &m.update {
        UpdateState::Disabled { unreadable: true } => {
            "Off: the settings file from IT (policy.ini) could not be read".into()
        }
        UpdateState::Disabled { unreadable: false } => "Managed by your IT department".into(),
        UpdateState::UpToDate => "Automatic when the app starts · up to date".into(),
        UpdateState::Available(r) => format!("Automatic when the app starts · version {} is available", r.version),
        UpdateState::Failed(reason) => format!("Automatic when the app starts · the last check failed: {reason}"),
        _ => "Automatic when the app starts".into(),
    }
}

/// How removing one old app went.
pub enum Removal {
    Removed,
    /// Its uninstaller ran but the app is still registered.
    Unfinished,
    /// It was open and didn't close within the wait; try again later.
    StillOpen,
    Failed(String),
}

fn names_text(names: &[&str]) -> String {
    let apps = if names.len() > 1 { "apps" } else { "app" };
    format!("{} {apps}", names.join(" and "))
}

/// Once the window is up: find the old separate apps and, the first time
/// this user opens the program, remove their copies. Their settings were
/// already copied when the settings were loaded at start-up.
pub fn check_old_apps() {
    let found = crate::old_apps::find();
    let due = found.iter().any(|a| a.removable()) && !with(|m| m.config.get().old_apps_handled).unwrap_or(true);
    with(|m| m.old_apps = found);
    super::relayout(PageId::Settings);
    if due {
        remove_old_apps(true);
    }
}

/// Run the old apps' uninstallers (this user's copies) in the background.
pub fn remove_old_apps(automatic: bool) {
    let Some((apps, events)) = with(|m| {
        if m.removing_old_apps {
            return None;
        }
        m.removing_old_apps = true;
        Some((m.old_apps.iter().filter(|a| a.removable()).cloned().collect::<Vec<_>>(), m.events.clone()))
    })
    .flatten() else {
        return;
    };
    super::sync(PageId::Settings);
    let main = super::main_handle();
    std::thread::spawn(move || {
        let results = apps
            .iter()
            .map(|a| {
                let outcome = match crate::old_apps::remove(a) {
                    Ok(true) => Removal::Removed,
                    Ok(false) => Removal::Unfinished,
                    Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => Removal::StillOpen,
                    Err(e) => Removal::Failed(e.to_string()),
                };
                (a.name, outcome)
            })
            .collect::<Vec<_>>();
        post(&events, main, Event::OldAppsRemoved { results, automatic });
    });
}

pub fn old_apps_removed(results: Vec<(&'static str, Removal)>, automatic: bool) {
    let found = crate::old_apps::find();
    with(|m| {
        m.removing_old_apps = false;
        m.old_apps = found;
    });
    let removed: Vec<&str> =
        results.iter().filter(|(_, r)| matches!(r, Removal::Removed)).map(|(name, _)| *name).collect();
    let open: Vec<&str> =
        results.iter().filter(|(_, r)| matches!(r, Removal::StillOpen)).map(|(name, _)| *name).collect();
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(name, r)| match r {
            Removal::Unfinished => Some(format!("{name}: its uninstaller did not finish.")),
            Removal::Failed(e) => Some(format!("{name}: {e}")),
            _ => None,
        })
        .collect();
    // An app that was still open is retried at the next start; anything
    // else is done (or for the user to finish in Settings).
    if open.is_empty() {
        super::update_settings(|s| s.old_apps_handled = true);
    }
    super::sync(PageId::Settings);

    let mut text = String::new();
    if !removed.is_empty() {
        text.push_str(&if automatic {
            format!(
                "{} replaces the separate {}. Your settings were copied over and the old {} been removed.",
                crate::app::DISPLAY_NAME,
                names_text(&removed),
                if removed.len() > 1 { "apps have" } else { "app has" },
            )
        } else {
            format!(
                "The separate {} {} been removed.",
                names_text(&removed),
                if removed.len() > 1 { "have" } else { "has" }
            )
        });
    }
    if !open.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "The {} {} still open, so {} couldn't be removed yet. Close {}; {} removed the next time you open {}.",
            names_text(&open),
            if open.len() > 1 { "are" } else { "is" },
            if open.len() > 1 { "they" } else { "it" },
            if open.len() > 1 { "them" } else { "it" },
            if open.len() > 1 { "they'll be" } else { "it'll be" },
            crate::app::DISPLAY_NAME,
        ));
    }
    if !failed.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "Some old apps could not be removed:\n\n{}\n\nYou can remove them in Windows Settings > Apps.",
            failed.join("\n")
        ));
    }
    if !text.is_empty() {
        let icon = if failed.is_empty() && open.is_empty() { MB_ICONINFORMATION } else { MB_ICONWARNING };
        message(&text, icon);
    }
}
