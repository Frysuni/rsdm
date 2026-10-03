//! Render a lock-screen preview PNG without a Wayland compositor.
//!
//! Usage:
//! `cargo run -p rsdm-lock --example preview -- [out.png] [theme] [preset] [border] [background] [frame] [wallpaper] [background_opacity]`

use std::path::PathBuf;

use rsdm_core::domain::{
    AppConfig, Background, BorderStyle, LogoPreset, PasswordRendering, ThemePreset, TitleSource,
};

// Preview the locker look. Edits the shared design under `lock.design`.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/rsdm-lock-preview.png"));
    let theme = args.next();
    let preset = args.next();
    let border = args.next();
    let background = args.next();
    let frame: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let wallpaper = args.next().filter(|path| path != "none");
    let background_opacity = args.next().and_then(|s| s.parse().ok());

    let mut config = AppConfig::default();
    config.lock.design.wallpaper = wallpaper;
    if let Some(opacity) = background_opacity {
        config.lock.design.background_opacity = opacity;
    }
    config.lock.design.password_mode = PasswordRendering::Asterisks;
    config.lock.design.theme = match theme.as_deref() {
        Some("minimal") => ThemePreset::Minimal,
        Some("monochrome") => ThemePreset::Monochrome,
        Some("gruvbox") => ThemePreset::Gruvbox,
        Some("nord") => ThemePreset::Nord,
        Some("cyberpunk") => ThemePreset::Cyberpunk,
        Some("dracula") => ThemePreset::Dracula,
        Some("tokyo-night") => ThemePreset::TokyoNight,
        Some("material") => ThemePreset::Material,
        Some("solarized") => ThemePreset::Solarized,
        Some("eldritch") => ThemePreset::Eldritch,
        Some("rama") => ThemePreset::Rama,
        Some("dark") => ThemePreset::Dark,
        Some("trans-is-hard-job") => ThemePreset::TransIsHardJob,
        _ => ThemePreset::Catppuccin,
    };
    config.lock.design.background = match background.as_deref() {
        Some("matrix") => Background::Matrix,
        Some("fire") => Background::Fire,
        Some("rain") => Background::Rain,
        Some("plasma") => Background::Plasma,
        Some("starfield") => Background::Starfield,
        _ => Background::None,
    };
    config.lock.design.border_style = match border.as_deref() {
        Some("none") => BorderStyle::None,
        Some("modern") => BorderStyle::Modern,
        Some("minimal") => BorderStyle::Minimal,
        Some("ascii1") | Some("block") => BorderStyle::Ascii1,
        Some("ascii2") | Some("gradient") => BorderStyle::Ascii2,
        Some("ascii3") | Some("panel") => BorderStyle::Ascii3,
        Some("ascii4") | Some("banner") => BorderStyle::Ascii4,
        Some("wave") => BorderStyle::Wave,
        Some("pulse") => BorderStyle::Pulse,
        _ => BorderStyle::Classic,
    };
    // With no preset arg, preview the generated ASCII-art title; a named preset
    // switches to the fixed distro logo.
    match preset.as_deref() {
        None => {
            config.lock.design.title_mode = TitleSource::Custom;
            config.lock.design.title_text = Some("RSDM".to_string());
        }
        Some(name) => {
            config.lock.design.title_mode = TitleSource::PresetLogo;
            config.lock.design.title_preset = match name {
                "niri" => LogoPreset::Niri,
                "hyprland" => LogoPreset::Hyprland,
                "kde" => LogoPreset::Kde,
                "gnome" => LogoPreset::Gnome,
                "arch" => LogoPreset::Arch,
                "nixos" => LogoPreset::Nixos,
                "tux" => LogoPreset::Tux,
                _ => LogoPreset::Rsdm,
            };
        }
    }

    rsdm_lock::preview_png(&config, 1280, 800, frame, &out)?;
    println!("wrote {}", out.display());
    Ok(())
}
