//! Shared screen composition over an abstract [`Surface`].

mod layout;
mod lock;
mod login;

use rsdm_core::domain::{Palette, Session};

use crate::backgrounds;
use crate::banner;
use crate::design::Design;
use crate::menu::Menu;
use crate::surface::{Rect, Surface};
use crate::text::{Role, Segment};

use layout::{content_width, draw_framed_box, push_banner, push_fields, too_small};
use lock::{draw_footer_lock, draw_status_lock, lock_too_small};
use login::{draw_footer_login, draw_picker, draw_status_login, login_fields};

/// Which input field has focus on the greeter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Username,
    Password,
    Session,
}

/// A destructive greeter action armed and awaiting a confirming second press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Exit,
    Reboot,
    Shutdown,
}

/// A power action on the locker, armed and awaiting a confirming second press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPending {
    Reboot,
    Shutdown,
    Hibernate,
    Sleep,
}

/// Smallest surface the box fits in; below this we show a notice (greeter) or a
/// minimal still-usable password prompt (locker).
pub const MIN_W: u16 = 44;
pub const MIN_H: u16 = 16;
const LABEL_WIDTH: usize = 9;
/// The value column reserves at least this many cells, so the fields do not
/// shift while the first ~10 characters are typed (the text grows centered into
/// the reserved width); past it the field widens.
const VALUE_SLOT: usize = 10;
const CONTENT_MIN: u16 = 34;
const CONTENT_MAX: u16 = 72;
/// The leading prompt/caret glyph showing where the next character lands.
const CARET: char = '_';

/// The dynamic content of one greeter frame (everything that is not look state).
pub struct LoginScene<'a> {
    pub title: Vec<String>,
    pub hostname: Option<String>,
    pub clock: Option<String>,
    pub username: &'a str,
    pub password_preview: String,
    pub field: Field,
    pub pending: Option<Pending>,
    pub console_exit_enabled: bool,
    /// Inline status under the fields: an error ("Authentication failed") or a
    /// progress note ("Authenticating...") depending on `message_is_error`.
    pub message: Option<&'a str>,
    pub message_is_error: bool,
    pub sessions: &'a [Session],
    pub selected: usize,
    pub session_field_visible: bool,
    pub session_picker_enabled: bool,
    pub picker_open: bool,
}

impl LoginScene<'_> {
    fn current_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }
}

/// The dynamic content of one locker frame.
pub struct LockScene<'a> {
    pub title: Vec<String>,
    pub hostname: Option<String>,
    pub clock: Option<String>,
    pub username: &'a str,
    pub password_preview: String,
    pub message: Option<&'a str>,
    pub message_is_error: bool,
    pub pending: Option<LockPending>,
    /// Whether the host can hibernate (so the footer offers it).
    pub hibernate_available: bool,
}

/// A line of the box body.
enum BodyLine {
    Blank,
    Centered(Vec<Segment>),
    /// One row of figlet title art. Every glyph shares the same cell pitch, so
    /// the block/ASCII art joins seamlessly on the framebuffer.
    Banner(String),
    Field {
        label: &'static str,
        value: String,
        focused: bool,
    },
}

// --- greeter --------------------------------------------------------------

/// Render the greeter. The front must have prepared the base layer first.
pub fn render_login(
    surface: &mut impl Surface,
    design: &Design,
    scene: &LoginScene<'_>,
    menu: &Menu,
    frame: u64,
) {
    let p = design.palette();
    let area = surface.area();
    if design.background.is_animated() {
        backgrounds::draw(
            surface,
            area,
            design.background,
            frame,
            design.background_speed,
            p,
        );
    }
    if area.w < MIN_W || area.h < MIN_H {
        too_small(surface, area, p);
        return;
    }

    // HUD (top status, bottom footer) first, each over a background-free margin;
    // then the opaque box over the center. They never overlap in normal sizes.
    draw_status_login(surface, area, scene, p);
    if menu.hints_enabled() || scene.pending.is_some() {
        draw_footer_login(surface, area, scene, p);
    }

    let fields = login_fields(scene);
    let content_w = content_width(&scene.title, &fields, scene.message, area.w);
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
        let line = if scene.message_is_error {
            Segment::bold(format!("x {message}"), Role::Danger)
        } else {
            Segment::bold(message.to_string(), Role::Info)
        };
        body.push(BodyLine::Blank);
        body.push(BodyLine::Centered(vec![line]));
    }
    body.push(BodyLine::Blank); // extra gap before the bottom border

    draw_framed_box(surface, design, area, content_w, &body, p, true);
    if scene.picker_open {
        draw_picker(surface, area, scene, p);
    }
    if menu.is_open() {
        menu.draw(surface, area, design, p);
    }
}

// --- locker ---------------------------------------------------------------

/// Render the locker. The front must have prepared the base layer first (e.g. a
/// wallpaper, or a clear to the theme background).
pub fn render_lock(
    surface: &mut impl Surface,
    design: &Design,
    scene: &LockScene<'_>,
    menu: &Menu,
    frame: u64,
) {
    let p = design.palette();
    let area = surface.area();
    render_lock_background(surface, design, area, p, frame);
    render_lock_content(surface, design, scene, menu, p, area, false);
}

/// Draw only the animated lock background. Lock front ends use this to blend the
/// background over a wallpaper before drawing the (transparent) UI on top.
pub fn render_lock_background(
    surface: &mut impl Surface,
    design: &Design,
    area: Rect,
    p: Palette,
    frame: u64,
) {
    if design.background.is_animated() {
        backgrounds::draw(
            surface,
            area,
            design.background,
            frame,
            design.background_speed,
            p,
        );
    }
}

/// Draw the lock content over whatever base layer the front prepared (a
/// wallpaper, an animated background, or a solid clear), without repainting it.
///
/// The box and HUD stay TRANSPARENT - drawing only their glyphs - whenever the
/// base carries real color the user wants to see behind the text: a wallpaper
/// (`has_wallpaper`) or a full-field effect like plasma/fire. Only a sparse
/// effect over the plain theme background (matrix/rain/starfield, no wallpaper)
/// fills an opaque box, where there is no colorful base to reveal and the fill
/// keeps the prompt readable over the moving dots.
pub fn render_lock_content(
    surface: &mut impl Surface,
    design: &Design,
    scene: &LockScene<'_>,
    menu: &Menu,
    p: Palette,
    area: Rect,
    has_wallpaper: bool,
) {
    if area.w < MIN_W || area.h < MIN_H {
        // Never lock the user out: drop the full UI but keep a usable prompt.
        lock_too_small(surface, area, scene, p);
        return;
    }

    // Fill an opaque box only when nothing colorful sits behind it.
    let opaque_regions = !has_wallpaper && !design.background.is_full_field();
    draw_status_lock(surface, area, scene, p, opaque_regions);
    if menu.hints_enabled() || scene.pending.is_some() {
        draw_footer_lock(surface, area, scene, p, opaque_regions);
    }

    let fields = vec![
        ("User", scene.username.to_string(), false),
        ("Password", scene.password_preview.clone(), true),
    ];
    let content_w = content_width(&scene.title, &fields, scene.message, area.w);
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
        body.push(BodyLine::Blank);
        body.push(BodyLine::Centered(vec![Segment::bold(
            message.to_string(),
            role,
        )]));
    }
    body.push(BodyLine::Blank); // extra gap before the bottom border

    draw_framed_box(surface, design, area, content_w, &body, p, opaque_regions);
    if menu.is_open() {
        menu.draw(surface, area, design, p);
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
