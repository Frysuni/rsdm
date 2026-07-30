//! Locker-only pieces of the screen: the top/footer HUD and the minimal
//! still-usable prompt shown when the surface is too small for the full UI.

use rsdm_core::domain::Palette;

use crate::surface::{Rect, Surface};
use crate::text::{Role, Segment};

use super::layout::{draw_bottom_hud, draw_top_hud, hint, key, sep};
use super::{CARET, LockPending, LockScene};

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
    power.push(sep());
    power.push(key("F10"));
    power.push(hint(" Sleep"));
    draw_bottom_hud(surface, area, &[primary, power], p, clear_background);
}

pub(super) fn lock_too_small(
    surface: &mut impl Surface,
    area: Rect,
    scene: &LockScene<'_>,
    p: Palette,
) {
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
    let prompt = format!("Password: {value}");
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
