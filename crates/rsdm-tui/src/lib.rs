//! Ratatui presentation for rsdm.
//!
//! The greeter is now a thin backend over the shared, medium-independent design
//! in `rsdm-ui`: the private `surface` module adapts `rsdm_ui::Surface` to ratatui's cell buffer,
//! and the run loop in [`screens`] drives input and hands a `LoginScene` to the
//! shared compositor. The look (themes, borders, backgrounds, the runtime menu)
//! lives in `rsdm-ui`, shared with the locker.

mod surface;

pub mod screens;

pub use screens::RatatuiLoginUi;
