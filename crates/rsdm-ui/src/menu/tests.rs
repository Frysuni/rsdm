use rsdm_core::domain::{Background, DesignConfig, SecondaryOutput};

use super::*;

fn design() -> Design {
    Design::from_config(&DesignConfig::default())
}

#[test]
fn closed_menu_ignores_keys() {
    let mut menu = Menu::new();
    assert_eq!(
        menu.handle_key(MenuKey::Down, &mut design()),
        MenuOutcome::Ignored
    );
}

#[test]
fn browsing_themes_applies_live() {
    let mut menu = Menu::new();
    let mut design = design();
    let starting_theme = design.theme;

    menu.open();
    menu.handle_key(MenuKey::Down, &mut design);
    menu.handle_key(MenuKey::Enter, &mut design);
    menu.handle_key(MenuKey::Down, &mut design);

    assert_ne!(design.theme, starting_theme);
    assert_eq!(
        menu.handle_key(MenuKey::Enter, &mut design),
        MenuOutcome::Closed
    );
}

#[test]
fn escape_returns_to_root_before_closing() {
    let mut menu = Menu::new();
    let mut design = design();
    menu.open();
    menu.handle_key(MenuKey::Down, &mut design);
    menu.handle_key(MenuKey::Enter, &mut design);

    assert_eq!(
        menu.handle_key(MenuKey::Esc, &mut design),
        MenuOutcome::Redraw
    );
    assert_eq!(
        menu.handle_key(MenuKey::Esc, &mut design),
        MenuOutcome::Closed
    );
}

#[test]
fn submenu_starts_on_the_active_item() {
    let config = DesignConfig {
        background: Background::Plasma,
        ..Default::default()
    };
    let mut design = Design::from_config(&config);
    let mut menu = Menu::new();
    menu.open();

    for _ in 0..3 {
        menu.handle_key(MenuKey::Down, &mut design);
    }
    menu.handle_key(MenuKey::Enter, &mut design);

    assert_eq!(design.background, Background::Plasma);
}

#[test]
fn wallpaper_controls_apply_live() {
    let mut menu = Menu::new();
    let mut design = design();
    menu.show_wallpaper_controls(true);
    menu.open();

    for _ in 0..6 {
        menu.handle_key(MenuKey::Down, &mut design);
    }
    menu.handle_key(MenuKey::Enter, &mut design);
    let starting_dim = design.dim;
    menu.handle_key(MenuKey::Down, &mut design);

    assert_ne!(design.dim, starting_dim);
    assert!(design.dim <= design::MAX_DIM);
}

#[test]
fn lock_controls_apply_live() {
    let mut menu = Menu::new();
    let mut design = design();
    menu.enable_lock_controls(LockMenuSettings {
        size: None,
        secondary_output: SecondaryOutput::Background,
    });
    menu.open();

    for _ in 0..ROOT_ROWS {
        menu.handle_key(MenuKey::Down, &mut design);
    }
    menu.handle_key(MenuKey::Enter, &mut design);
    menu.handle_key(MenuKey::Down, &mut design);
    assert_eq!(menu.lock_settings().expect("lock settings").size, Some(1));

    menu.handle_key(MenuKey::Esc, &mut design);
    for _ in 0..ROOT_ROWS + 1 {
        menu.handle_key(MenuKey::Down, &mut design);
    }
    menu.handle_key(MenuKey::Enter, &mut design);
    menu.handle_key(MenuKey::Down, &mut design);
    assert_eq!(
        menu.lock_settings()
            .expect("lock settings")
            .secondary_output,
        SecondaryOutput::Black
    );
}

#[test]
fn save_row_requests_persistence_without_closing() {
    let mut menu = Menu::new();
    let mut design = design();
    menu.enable_lock_controls(LockMenuSettings {
        size: Some(2),
        secondary_output: SecondaryOutput::Black,
    });
    menu.open();
    for _ in 0..ROOT_ROWS + 2 {
        menu.handle_key(MenuKey::Down, &mut design);
    }

    assert_eq!(
        menu.handle_key(MenuKey::Enter, &mut design),
        MenuOutcome::SaveRequested
    );
    assert!(menu.is_open());
}
