//! Shared, medium-independent rendering for the greeter and locker.

pub mod backgrounds;
pub mod banner;
pub mod borders;
pub mod design;
pub mod menu;
pub mod screen;
pub mod surface;
pub mod text;

pub use design::{Design, MAX_DIM, MAX_OPACITY, MAX_SPEED};
pub use menu::{LockMenuSettings, Menu, MenuKey, MenuOutcome};
pub use screen::{
    Field, LockPending, LockScene, LoginScene, Pending, render_lock, render_lock_background,
    render_lock_content, render_login,
};
pub use surface::{Cell, Rect, Surface};
