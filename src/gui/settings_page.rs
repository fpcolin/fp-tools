//! "Settings": everything that is remembered, per user.

use super::canvas::RectF;
use super::layout::{Frame, Item, text_height, text_width};
use super::qr_page;
use super::theme::{ButtonKind, Font, Icon, c};
use super::updates::{self, UpdateState};
use super::win32::*;
use super::{Model, PageId, Ui, ui, update_settings, widgets, with};
use crate::APP;

const ID_CHANGE_FOLDER: u16 = 400;
const ID_FILE_NAME: u16 = 401;
const ID_LOGO_CHOOSE: u16 = 402;
const ID_LOGO_DEFAULT: u16 = 403;
const ID_EMBED: u16 = 404;
const ID_ROUND: u16 = 405;
const ID_OPEN_IMAGE: u16 = 406;
const ID_OPEN_FOLDER: u16 = 407;
const ID_VC_OVERWRITE: u16 = 408;
const ID_VC_OPEN: u16 = 409;
const ID_CHECK_NOW: u16 = 410;
const ID_REMOVE_OLD: u16 = 411;

const NAME_HINT: &str = "If a file with this name is already there, (1), (2)… is added instead of replacing it.";

pub struct Controls {
    pub folder: HWND,
    pub change_folder: HWND,
    pub name_label: HWND,
    pub name: HWND,
    pub name_hint: HWND,
    pub logo_name: HWND,
    pub logo_choose: HWND,
    pub logo_default: HWND,
    pub embed: HWND,
    pub round: HWND,
    pub open_image: HWND,
    pub open_folder: HWND,
    pub vc_overwrite: HWND,
    pub vc_open: HWND,
    pub check_now: HWND,
    pub old_text: HWND,
    pub remove_old: HWND,
}

pub fn create(page: HWND) -> Controls {
    let w = c::WHITE;
    let folder = widgets::path_label(page, Font::Body, c::TEXT, w);
    let change_folder = widgets::button(page, ID_CHANGE_FOLDER, "Change…", ButtonKind::Secondary, w);
    let name_label = widgets::mnemonic_label(page, "File &name", Font::Body, c::MUTED, w);
    let name = widgets::edit(page, ID_FILE_NAME, 150);
    let name_hint = widgets::label(page, NAME_HINT, Font::Small, c::MUTED, w);
    let logo_name = widgets::label_styled(page, "", Font::Body, c::TEXT, w, SS_NOPREFIX | SS_PATHELLIPSIS);
    let logo_choose = widgets::button(page, ID_LOGO_CHOOSE, "Use my own image…", ButtonKind::Secondary, w);
    let logo_default = widgets::button(page, ID_LOGO_DEFAULT, "Reset to default", ButtonKind::Secondary, w);
    let embed = widgets::checkbox(page, ID_EMBED, "Put a logo in the middle of QR codes", w);
    let round = widgets::checkbox(page, ID_ROUND, "Rounded corners", w);
    let open_image = widgets::checkbox(page, ID_OPEN_IMAGE, "Open the image after saving", w);
    let open_folder = widgets::checkbox(page, ID_OPEN_FOLDER, "Show it in its folder after saving", w);
    let vc_overwrite = widgets::checkbox(page, ID_VC_OVERWRITE, "Replace the exported file when converting", w);
    let vc_open = widgets::checkbox(page, ID_VC_OPEN, "Show the converted file in its folder", w);
    let check_now = widgets::button(page, ID_CHECK_NOW, "Check now", ButtonKind::Secondary, w);
    let old_text = widgets::label(page, "", Font::Body, c::TEXT, w);
    let remove_old = widgets::button(page, ID_REMOVE_OLD, "Remove old apps", ButtonKind::Secondary, w);
    Controls {
        folder,
        change_folder,
        name_label,
        name,
        name_hint,
        logo_name,
        logo_choose,
        logo_default,
        embed,
        round,
        open_image,
        open_folder,
        vc_overwrite,
        vc_open,
        check_now,
        old_text,
        remove_old,
    }
}

fn old_apps_text(m: &Model) -> String {
    if m.removing_old_apps {
        return "Removing the old apps…".into();
    }
    let mine: Vec<&str> = m.old_apps.iter().filter(|a| a.removable()).map(|a| a.name).collect();
    let all_users: Vec<&str> = m.old_apps.iter().filter(|a| a.all_users).map(|a| a.name).collect();
    let mut text = String::new();
    if !mine.is_empty() {
        text.push_str(&format!(
            "The separate {} app{} {} still installed for you. This app replaces {}, and your settings were copied over.",
            mine.join(" and "),
            if mine.len() > 1 { "s" } else { "" },
            if mine.len() > 1 { "are" } else { "is" },
            if mine.len() > 1 { "them" } else { "it" },
        ));
    }
    if !all_users.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!(
            "{} {} installed for everyone on this PC; an administrator can remove {} in Windows Settings > Apps.",
            all_users.join(" and "),
            if all_users.len() > 1 { "are" } else { "is" },
            if all_users.len() > 1 { "them" } else { "it" },
        ));
    }
    text
}

pub fn sync() {
    let s = &ui().settings;
    let Some((cfg, old_text, can_remove, removing)) = with(|m| {
        (m.config.get().clone(), old_apps_text(m), m.old_apps.iter().any(|a| a.removable()), m.removing_old_apps)
    }) else {
        return;
    };
    let qr = &cfg.qr;
    let folder = if qr.saveto_path.trim().is_empty() {
        crate::common::paths::downloads().to_string_lossy().into_owned()
    } else {
        qr.saveto_path.clone()
    };
    widgets::set_text(s.folder, &folder);
    if widgets::focused() != s.name || !widgets::modified(s.name) {
        widgets::set_text(s.name, &qr.saveto_name);
        widgets::clear_modified(s.name);
    }
    let status = qr_page::logo_status(&qr.embed_image);
    widgets::set_text(s.logo_name, &status.short_text());
    super::theme::set_colors(s.logo_name, status.text_color(true), c::WHITE);
    // Always there, so it's clear the default can be brought back; only
    // clickable while another image is in use.
    widgets::enable(s.logo_default, status.custom);
    widgets::set_checked(s.embed, qr.option_embed_logo);
    widgets::set_checked(s.round, qr.option_round_edges);
    widgets::set_checked(s.open_image, qr.option_open_image);
    widgets::set_checked(s.open_folder, qr.option_open_folder);
    widgets::set_checked(s.vc_overwrite, cfg.vc.option_overwrite);
    widgets::set_checked(s.vc_open, cfg.vc.option_open_folder);
    widgets::set_text(s.old_text, &old_text);
    widgets::enable(s.remove_old, can_remove && !removing);
}

/// Label column + content column rows inside a card.
const LABEL_W: f32 = 120.0;

pub fn layout(ui: &Ui, m: &mut Model, width: f32) -> Frame {
    let s = &ui.settings;
    let cfg = m.config.get().clone();
    let mut f = Frame::default();
    let x = 40.0;
    let avail = (width - 80.0).clamp(400.0, 820.0);
    f.text(RectF::new(x, 28.0, avail, 36.0), "Settings", Font::Title, c::NAVY, DT_SINGLELINE);
    f.text(
        RectF::new(x, 68.0, avail, 20.0),
        "These are yours only. Other people who use this PC keep their own.",
        Font::Body,
        c::MUTED,
        DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    let (x0, iw) = (x + 28.0, avail - 56.0);
    let cx = x0 + LABEL_W;
    let cw = iw - LABEL_W;
    let row_label = |f: &mut Frame, y: f32, text: &str| {
        f.text(RectF::new(x0, y, LABEL_W - 12.0, 20.0), text, Font::Body, c::MUTED, DT_SINGLELINE);
    };

    // QR codes card.
    let card_top = 112.0;
    let mark = f.mark();
    let mut y = card_top + 24.0;
    f.text(RectF::new(x0, y, iw, 22.0), "QR codes", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 40.0;
    row_label(&mut f, y + 8.0, "Save to");
    let change_w = text_width(Font::Body, "Change…") + 36.0;
    f.place(s.folder, RectF::new(cx, y + 8.0, cw - change_w - 16.0, 20.0));
    f.place(s.change_folder, RectF::new(x0 + iw - change_w, y, change_w, 36.0));
    y += 52.0;
    f.place(s.name_label, RectF::new(x0, y + 10.0, LABEL_W - 12.0, 20.0));
    let name_w = cw.min(300.0);
    f.field(s.name, RectF::new(cx, y, name_w - 44.0, 40.0));
    f.text(RectF::new(cx + name_w - 36.0, y + 10.0, 40.0, 20.0), ".png", Font::Body, c::MUTED, DT_SINGLELINE);
    y += 46.0;
    let hint_h = text_height(Font::Small, NAME_HINT, cw);
    f.place(s.name_hint, RectF::new(cx, y, cw, hint_h));
    y += hint_h + 22.0;
    row_label(&mut f, y + 14.0, "Logo");
    let thumb = RectF::new(cx, y, 48.0, 48.0);
    f.panel(thumb, c::WHITE, c::BORDER, 10.0);
    let missing = qr_page::logo_status(&cfg.qr.embed_image).missing;
    match (!missing).then(|| super::logo_bitmap(m, &cfg.qr.embed_image, super::theme::px(40.0), c::WHITE)).flatten() {
        Some(bmp) => f.items.push(Item::Image(RectF::new(cx + 4.0, y + 4.0, 40.0, 40.0), bmp)),
        None => f.items.push(Item::Icon(RectF::new(cx + 12.0, y + 12.0, 24.0, 24.0), Icon::Alert, c::ERROR)),
    }
    let choose_w = text_width(Font::Body, "Use my own image…") + 36.0;
    let default_w = text_width(Font::Body, "Reset to default") + 36.0;
    f.place(s.logo_name, RectF::new(cx + 64.0, y + 2.0, cw - 64.0, 20.0));
    f.place(s.logo_choose, RectF::new(cx + 64.0, y + 26.0, choose_w, 36.0));
    f.place(s.logo_default, RectF::new(cx + 64.0 + choose_w + 10.0, y + 26.0, default_w, 36.0));
    y += 78.0;
    for cb in [s.embed, s.round, s.open_image, s.open_folder] {
        f.place(cb, RectF::new(cx, y, cw, 24.0));
        y += 34.0;
    }
    y += 10.0;
    f.card_behind(mark, RectF::new(x, card_top, avail, y - card_top));

    // Converter card.
    let card_top = y + 20.0;
    let mark = f.mark();
    let mut y = card_top + 24.0;
    f.text(RectF::new(x0, y, iw, 22.0), "VendorCafe converter", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 40.0;
    for cb in [s.vc_overwrite, s.vc_open] {
        f.place(cb, RectF::new(x0, y, iw, 24.0));
        y += 34.0;
    }
    y += 10.0;
    f.card_behind(mark, RectF::new(x, card_top, avail, y - card_top));

    // Updates card.
    let card_top = y + 20.0;
    let mark = f.mark();
    let mut y = card_top + 24.0;
    f.text(RectF::new(x0, y, iw, 22.0), "Updates", Font::Heading, c::NAVY, DT_SINGLELINE);
    y += 40.0;
    let checks = updates::checks_text(m);
    let rows = [
        ("Version", APP.version.to_string()),
        ("Installed for", updates::scope_text(&m.policy).to_string()),
        ("Update checks", checks),
    ];
    for (label, value) in rows {
        row_label(&mut f, y, label);
        let h = text_height(Font::Body, &value, cw).max(20.0);
        f.text(RectF::new(cx, y, cw, h), &value, Font::Body, c::TEXT, DT_WORDBREAK);
        y += h + 12.0;
    }
    let enabled = !matches!(m.update, UpdateState::Disabled { .. });
    let check_w = text_width(Font::Body, "Check now") + 36.0;
    f.show_if(s.check_now, enabled, RectF::new(cx, y + 2.0, check_w, 36.0));
    if enabled {
        y += 50.0;
    }
    if m.policy.scope == crate::common::policy::InstallScope::Machine {
        let hint = "Installing an update for everyone on this PC needs an administrator's approval.";
        let h = text_height(Font::Small, hint, cw);
        f.text(RectF::new(cx, y, cw, h), hint, Font::Small, c::MUTED, DT_WORDBREAK);
        y += h + 8.0;
    }
    y += 16.0;
    f.card_behind(mark, RectF::new(x, card_top, avail, y - card_top));

    // Old apps card, while any are installed.
    let text = old_apps_text(m);
    if text.is_empty() {
        f.hide(s.old_text);
        f.hide(s.remove_old);
    } else {
        let card_top = y + 20.0;
        let mark = f.mark();
        let mut y = card_top + 24.0;
        f.text(RectF::new(x0, y, iw, 22.0), "Old apps", Font::Heading, c::NAVY, DT_SINGLELINE);
        y += 40.0;
        let h = text_height(Font::Body, &text, iw);
        f.place(s.old_text, RectF::new(x0, y, iw, h));
        y += h + 16.0;
        let any_mine = m.old_apps.iter().any(|a| a.removable());
        let remove_w = text_width(Font::Body, "Remove old apps") + 36.0;
        f.show_if(s.remove_old, any_mine, RectF::new(x0, y, remove_w, 36.0));
        if any_mine {
            y += 52.0;
        }
        y += 8.0;
        f.card_behind(mark, RectF::new(x, card_top, avail, y - card_top));
        f.height = y + 32.0;
        return f;
    }
    f.height = y + 32.0;
    f
}

/// Save the file name typed so far (called when focus leaves the box, when
/// switching pages and before closing).
pub fn commit_file_name() {
    let s = &ui().settings;
    // Only what the user typed: the box may not have been filled in yet.
    if !widgets::modified(s.name) {
        return;
    }
    widgets::clear_modified(s.name);
    let typed = widgets::text(s.name);
    let name = typed.trim();
    let name = if name.is_empty() { crate::config::DEFAULT_FILE_NAME } else { name }.to_string();
    let changed = with(|m| m.config.get().qr.saveto_name != name).unwrap_or(false);
    if changed {
        update_settings(|st| st.qr.saveto_name = name);
        super::sync(PageId::Qr);
    }
}

pub fn command(id: u16, code: u16) {
    let s = &ui().settings;
    if id == ID_FILE_NAME {
        if code == EN_KILLFOCUS {
            commit_file_name();
            super::sync(PageId::Settings);
        }
        return;
    }
    if code != BN_CLICKED {
        return;
    }
    let flag = |hwnd: HWND| widgets::checked(hwnd);
    match id {
        ID_CHANGE_FOLDER => {
            commit_file_name();
            qr_page::choose_save_location();
        }
        ID_LOGO_CHOOSE => qr_page::choose_logo(),
        ID_LOGO_DEFAULT => qr_page::use_default_logo(),
        ID_EMBED | ID_ROUND | ID_OPEN_IMAGE | ID_OPEN_FOLDER => {
            let (embed, round, open_image, open_folder) =
                (flag(s.embed), flag(s.round), flag(s.open_image), flag(s.open_folder));
            update_settings(|st| {
                st.qr.option_embed_logo = embed;
                st.qr.option_round_edges = round;
                st.qr.option_open_image = open_image;
                st.qr.option_open_folder = open_folder;
            });
            qr_page::settings_changed();
        }
        ID_VC_OVERWRITE | ID_VC_OPEN => {
            let (overwrite, open) = (flag(s.vc_overwrite), flag(s.vc_open));
            update_settings(|st| {
                st.vc.option_overwrite = overwrite;
                st.vc.option_open_folder = open;
            });
            super::sync(PageId::Converter);
        }
        ID_CHECK_NOW => updates::check_now(),
        ID_REMOVE_OLD => updates::remove_old_apps(false),
        _ => {}
    }
}
