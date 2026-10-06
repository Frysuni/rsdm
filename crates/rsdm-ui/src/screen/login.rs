//! Greeter-only pieces of the screen: the field list, the top/footer HUD and the
//! session picker modal.

use rsdm_core::domain::Palette;

use crate::surface::{Rect, Surface};
use crate::text::{Role, Segment, draw_segments_centered};

use super::layout::{draw_bottom_hud, draw_double_frame, draw_top_hud, hint, key, sep};
use super::{Field, LoginScene, Pending};

pub(super) fn login_fields(scene: &LoginScene<'_>) -> Vec<(&'static str, String, bool)> {
    let mut fields = vec![
        (
            "User",
            scene.username.to_string(),
            scene.field == Field::Username,
        ),
        (
            "Password",
            scene.password_preview.clone(),
            scene.field == Field::Password,
        ),
    ];
    if scene.session_field_visible {
        let value = scene
            .current_session()
            .map_or(String::new(), |s| format!("{} ({})", s.name, s.id));
        fields.push(("Session", value, scene.field == Field::Session));
    }
    fields
}

pub(super) fn draw_status_login(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LoginScene<'_>,
    p: Palette,
) {
    let mut segs: Vec<Segment> = Vec::new();
    let sep = || Segment::new("  -  ", Role::FgMuted);
    if let Some(hostname) = &scene.hostname {
        segs.push(Segment::new(hostname.clone(), Role::FgSecondary));
    }
    if let Some(clock) = &scene.clock {
        if !segs.is_empty() {
            segs.push(sep());
        }
        segs.push(Segment::new(clock.clone(), Role::Secondary));
    }
    if scene.session_field_visible
        && let Some(session) = scene.current_session()
    {
        if !segs.is_empty() {
            segs.push(sep());
        }
        segs.push(Segment::new(
            format!("session: {}", session.name),
            Role::FgSecondary,
        ));
    }
    draw_top_hud(surface, area, &segs, p, true);
}

pub(super) fn draw_footer_login(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LoginScene<'_>,
    p: Palette,
) {
    if let Some(pending) = scene.pending {
        let (label, word) = match pending {
            Pending::Exit => ("Esc", "exit to the console"),
            Pending::Reboot => ("F11", "reboot"),
            Pending::Shutdown => ("F12", "shut down"),
        };
        let line = vec![
            Segment::bold(format!("Press {label} again to {word}"), Role::Danger),
            Segment::new("   -   any other key cancels", Role::FgMuted),
        ];
        draw_bottom_hud(surface, area, &[line], p, true);
        return;
    }

    let mut primary = vec![
        key("Enter"),
        hint(" Login"),
        sep(),
        key("Tab/Up/Dn"),
        hint(" Field"),
        sep(),
        key("F1"),
        hint(" Menu"),
    ];
    if scene.session_picker_enabled {
        primary.push(sep());
        primary.push(key("F2"));
        primary.push(hint(" Sessions"));
    }
    let mut secondary = Vec::new();
    if scene.console_exit_enabled {
        secondary.extend([key("Esc"), hint(" Exit"), sep()]);
    }
    secondary.extend([
        key("F11"), hint(" Reboot"), sep(), key("F12"), hint(" Shutdown"),
    ]);
    draw_bottom_hud(surface, area, &[primary, secondary], p, true);
}

pub(super) fn draw_picker(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LoginScene<'_>,
    p: Palette,
) {
    let rows: Vec<String> = scene
        .sessions
        .iter()
        .map(|s| {
            let comment = s
                .comment
                .as_deref()
                .map(|c| format!(" - {c}"))
                .unwrap_or_default();
            format!("{} ({}){comment}", s.name, s.id)
        })
        .collect();
    let label_w = rows
        .iter()
        .map(|r| r.chars().count() as u16)
        .max()
        .unwrap_or(0)
        .max(10);
    let w = (label_w + 6).min(area.w);
    let h = (rows.len() as u16 + 5).min(area.h);
    let modal = area.centered(w, h);
    surface.fill(modal, p.bg_base);
    let title = [Segment::bold(" sessions ", Role::Accent)];
    draw_double_frame(surface, modal, p.accent);
    draw_segments_centered(surface, modal.center_x(), modal.y + 1, &title, p);
    for (i, label) in rows.iter().enumerate() {
        let y = modal.y + 3 + i as u16;
        if y >= modal.bottom() - 1 {
            break;
        }
        let selected = i == scene.selected;
        let marker = if selected { "> " } else { "  " };
        let color = if selected { p.accent } else { p.fg_secondary };
        surface.text(modal.x + 2, y, &format!("{marker}{label}"), color, selected);
    }
}
