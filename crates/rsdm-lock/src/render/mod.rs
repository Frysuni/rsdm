//! Pure software rendering for the locker. No Wayland types appear here, so the
//! whole module is unit-testable on a headless host.
//!
//! `canvas` and `font` are the pixel primitives; `surface` adapts them to the
//! shared `rsdm_ui::Surface` so the locker draws the same composition as the
//! greeter. Backgrounds and borders live in `rsdm-ui` now, not here.

pub mod canvas;
pub mod font;
pub mod surface;
mod wallpaper;

pub use canvas::Canvas;
pub use font::Font;
pub use surface::{FbSurface, resolve_zoom};
pub use wallpaper::{Wallpaper, tint};
