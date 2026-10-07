//! Locker-only pieces of the screen: the top/footer HUD and the minimal
//! still-usable prompt shown when the surface is too small for the full UI.

use rsdm_core::domain::Palette;

use crate::{banner, design::Design};
use crate::surface::{Rect, Surface};
use crate::text::{Role, Segment};

use super::message::push_message;
use super::layout::{
    draw_bottom_hud, draw_top_hud, hint, key, push_banner, push_fields, sep,
};
use super::{BodyLine, CARET, LockPending, LockScene};

pub(super) fn draw_status_lock(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LockScene<'_>,
    p: Palette,
    clear_background: bool,
) {
    let mut segs: Vec<Segment> = Vec::new();
    if let Some(hostname) = &scene.hostname {
        segs.push(Segment::new(hostname.clone(), Role::FgSecondary));
    }
    if let Some(clock) = &scene.clock {
        if !segs.is_empty() {
            segs.push(Segment::new("  -  ", Role::FgMuted));
        }
        segs.push(Segment::new(clock.clone(), Role::Secondary));
    }
    draw_top_hud(surface, area, &segs, p, clear_background);
}

pub(super) fn draw_footer_lock(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LockScene<'_>,
    p: Palette,
    clear_background: bool,
) {
    if scene.authentication_active {
        let line = vec![
            key("Enter"), hint(" Continue"), sep(), key("Esc"), hint(" Cancel"),
        ];
        draw_bottom_hud(surface, area, &[line], p, clear_background);
        return;
    }

    if let Some(pending) = scene.pending {
        let (label, word) = match pending {
            LockPending::Reboot => ("F11", "reboot"),
            LockPending::Shutdown => ("F12", "shut down"),
            LockPending::Hibernate => ("F9", "hibernate"),
            LockPending::Sleep => ("F10", "sleep"),
        };
        let line = vec![
            Segment::bold(format!("Press {label} again to {word}"), Role::Danger),
            Segment::new("   -   any other key cancels", Role::FgMuted),
        ];
        draw_bottom_hud(surface, area, &[line], p, clear_background);
        return;
    }

    let primary = vec![
        key("Enter"),
        hint(" Unlock"),
        sep(),
        key("F1"),
        hint(" Menu"),
        sep(),
        key("Esc"),
        hint(" Clear"),
    ];
    let mut power = vec![
        key("F11"),
        hint(" Reboot"),
        sep(),
        key("F12"),
        hint(" Shutdown"),
    ];
    if scene.hibernate_available {
        power.push(sep());
        power.push(key("F9"));
        power.push(hint(" Hibernate"));
    }
    if scene.suspend_available {
        power.push(sep());
        power.push(key("F10"));
        power.push(hint(" Sleep"));
    }
    draw_bottom_hud(surface, area, &[primary, power], p, clear_background);
}

pub(super) fn lock_too_small(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LockScene<'_>,
    p: Palette,
) {
    if scene.authentication_active {
        super::authentication::draw_prompt(
            surface, area, &scene.password_preview, scene.message, scene.message_is_error, p,
        );
        return;
    }
    let cx = area.center_x();
    let cy = area.center_y();
    surface.text_centered(
        cx,
        cy.saturating_sub(1),
        "screen too small",
        p.warning,
        true,
    );
    // A still-usable password prompt so the session can always be unlocked.
    let mut value = scene.password_preview.clone();
    value.push(CARET);
    let label = if scene.authentication_active { "Response" } else { "Password" };
    let prompt = format!("{label}: {value}");
    surface.text_centered(cx, cy, &prompt, p.fg_primary, true);
    if let Some(message) = scene.message {
        let color = if scene.message_is_error {
            p.danger
        } else {
            p.secondary
        };
        surface.text_centered(cx, cy + 1, message, color, true);
    } else {
        surface.text_centered(cx, cy + 1, "Enter to unlock", p.fg_secondary, false);
    }
}

pub(super) fn lock_body(
    scene: &LockScene<'_>,
    fields: Vec<(&'static str, String, bool)>,
    content_w: u16,
    design: &Design,
) -> Vec<BodyLine> {
    let mut body = vec![
        BodyLine::Centered(vec![Segment::bold(
            banner::caption("LOCKED", content_w as usize, design.border),
            Role::Accent,
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
            Role::Secondary
        };
        push_message(&mut body, message, role, content_w);
    }
    body.push(BodyLine::Blank); // extra gap before the bottom border

    body
}
