//! Greeter-only pieces of the screen: the field list, the top/footer HUD and the
//! session picker modal.

use rsdm_core::domain::Palette;

use crate::{banner, design::Design};
use crate::surface::{Rect, Surface};
use crate::text::{Role, Segment, draw_segments_centered};

use super::message::push_message;
use super::layout::{
    draw_bottom_hud, draw_double_frame, draw_top_hud, hint, key, push_banner, push_fields, sep,
};
use super::{BodyLine, Field, LoginScene, Pending};

pub(super) fn login_fields(scene: &LoginScene<'_>) -> Vec<(&'static str, String, bool)> {
    let mut fields = vec![
        (
            "User",
            scene.username.to_string(),
            scene.field == Field::Username,
        ),
        (
            if scene.authentication_active {
                "Response"
            } else {
                "Password"
            },
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
    if scene.authentication_active {
        let line = vec![
            key("Enter"), hint(" Continue"), sep(), key("Esc"), hint(" Cancel"),
        ];
        draw_bottom_hud(surface, area, &[line], p, true);
        return;
    }

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
    let visible_rows = usize::from(modal.h.saturating_sub(4));
    let first = scene.selected.saturating_sub(visible_rows / 2)
        .min(rows.len().saturating_sub(visible_rows));
    for (row, label) in rows.iter().skip(first).take(visible_rows).enumerate() {
        let i = first + row;
        let y = modal.y + 3 + row as u16;
        let selected = i == scene.selected;
        let marker = if selected { "> " } else { "  " };
        let color = if selected { p.accent } else { p.fg_secondary };
        surface.text(modal.x + 2, y, &format!("{marker}{label}"), color, selected);
    }
}

pub(super) fn login_body(
    scene: &LoginScene<'_>,
    fields: Vec<(&'static str, String, bool)>,
    content_w: u16,
    design: &Design,
) -> Vec<BodyLine> {
    let mut body = vec![
        BodyLine::Centered(vec![Segment::bold(
            banner::caption("LOGIN", content_w as usize, design.border),
            Role::Primary,
        )]),
        BodyLine::Blank,
    ];
    push_banner(&mut body, &scene.title);
    body.push(BodyLine::Blank);
    push_fields(&mut body, fields);
    if let Some(message) = scene.message {
        let role = if scene.message_is_error {
            Role::Danger
        } else {
            Role::Info
        };
        let text = if scene.message_is_error { format!("x {message}") } else { message.to_string() };
        push_message(&mut body, &text, role, content_w);
    }
    body.push(BodyLine::Blank); // extra gap before the bottom border

    body
}
