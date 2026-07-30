//! ASCII logo art shared by every rsdm front end (TUI greeter and locker).
//!
//! The art is pure data: it carries no rendering dependency, so both the
//! ratatui greeter and the Wayland locker can lay it out in their own way while
//! staying visually identical.

use super::LogoPreset;

/// Render the lines for `preset`.
///
/// `fallback` is used verbatim for [`LogoPreset::Minimal`] so callers can feed
/// a hostname or custom title through the same code path as the figlet presets.
pub fn logo(preset: LogoPreset, fallback: &str) -> Vec<String> {
    match preset {
        LogoPreset::Minimal => vec![fallback.to_string()],
        LogoPreset::Block | LogoPreset::Rsdm => owned(RSDM),
        LogoPreset::Rustty => owned(RUSTTY),
        LogoPreset::Waytty => owned(WAYTTY),
        LogoPreset::Niri => owned(NIRI),
        LogoPreset::Hyprland => owned(HYPRLAND),
        LogoPreset::Kde => owned(KDE),
        LogoPreset::Gnome => owned(GNOME),
        LogoPreset::Nixos => owned(NIXOS),
        LogoPreset::Arch => owned(ARCH),
        LogoPreset::Tux => owned(TUX),
    }
}

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_string()).collect()
}

const RSDM: &[&str] = &[
    " ____   ____ _____ ____  __  __ ",
    "|  _ \\ / ___|_   _|  _ \\|  \\/  |",
    "| |_) |\\___ \\ | | | | | | |\\/| |",
    "|  _ <  ___) || | | |_| | |  | |",
    "|_| \\_\\|____/ |_| |____/|_|  |_|",
];

const RUSTTY: &[&str] = &[
    " ____  _   _ ____ _____ _______   __",
    "|  _ \\| | | / ___|_   _|_   _\\ \\ / /",
    "| |_) | | | \\___ \\ | |   | |  \\ V / ",
    "|  _ <| |_| |___) || |   | |   | |  ",
    "|_| \\_\\\\___/|____/ |_|   |_|   |_|  ",
];

const WAYTTY: &[&str] = &[
    "__        ___ __   _______ _______   __",
    "\\ \\      / / \\\\ \\ / /_   _|_   _\\ \\ / /",
    " \\ \\ /\\ / / _ \\\\ V /  | |   | |  \\ V / ",
    "  \\ V  V / ___ \\| |   | |   | |   | |  ",
    "   \\_/\\_/_/   \\_\\_|   |_|   |_|   |_|  ",
];

const NIRI: &[&str] = &[
    " _   _ ___ ____  ___",
    "| \\ | |_ _|  _ \\|_ _|",
    "|  \\| || || |_) || | ",
    "| |\\  || ||  _ < | | ",
    "|_| \\_|___|_| \\_\\___|",
];

const HYPRLAND: &[&str] = &[
    " _   ___   ______  ____  _     ___    _   _ ____ ",
    "| | | \\ \\ / /  _ \\|  _ \\| |   / _ \\  | \\ | |  _ \\",
    "| |_| |\\ V /| |_) | |_) | |  | |_| | |  \\| | | | |",
    "|  _  | | | |  __/|  _ <| |  |  _  | | |\\  | |_| |",
    "|_| |_| |_| |_|   |_| \\_\\_|  |_| |_| |_| \\_|____/ ",
];

const KDE: &[&str] = &[
    " _  ______  _____ ",
    "| |/ /  _ \\| ____|",
    "| ' /| | | |  _|  ",
    "| . \\| |_| | |___ ",
    "|_|\\_\\____/|_____|",
];

const GNOME: &[&str] = &[
    "  ____ _   _  ___  __  __ _____ ",
    " / ___| \\ | |/ _ \\|  \\/  | ____|",
    "| |  _|  \\| | | | | |\\/| |  _|  ",
    "| |_| | |\\  | |_| | |  | | |___ ",
    " \\____|_| \\_|\\___/|_|  |_|_____|",
];

const NIXOS: &[&str] = &[
    " _   _ _____  _____ ___  ____",
    "| \\ | |_ _\\ \\/ / _ \\/ ___|",
    "|  \\| || | \\  / | | \\___ \\",
    "| |\\  || | /  \\ |_| |___) |",
    "|_| \\_|___/_/\\_\\___/|____/",
];

const ARCH: &[&str] = &[
    "    /\\    ____   ____ _   _",
    "   /  \\  |  _ \\ / ___| | | |",
    "  / /\\ \\ | |_) | |   | |_| |",
    " / ____ \\|  _ <| |___|  _  |",
    "/_/    \\_\\_| \\_\\\\____|_| |_|",
];

const TUX: &[&str] = &[
    "   .--.   ",
    "  |o_o |  ",
    "  |:_/ |  ",
    " //   \\ \\ ",
    "(|     | )",
    "/'\\_   _/`\\",
    "\\___)=(___/",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LogoPreset;

    #[test]
    fn minimal_uses_the_fallback_verbatim() {
        assert_eq!(
            logo(LogoPreset::Minimal, "myhost"),
            vec!["myhost".to_string()]
        );
    }

    #[test]
    fn figlet_presets_have_art() {
        for preset in [
            LogoPreset::Rsdm,
            LogoPreset::Rustty,
            LogoPreset::Waytty,
            LogoPreset::Niri,
            LogoPreset::Hyprland,
            LogoPreset::Kde,
            LogoPreset::Gnome,
            LogoPreset::Nixos,
            LogoPreset::Arch,
            LogoPreset::Tux,
        ] {
            let art = logo(preset, "ignored");
            assert!(art.len() > 1, "{preset:?} should be multi-line");
            assert!(
                art.iter().any(|line| !line.is_empty()),
                "{preset:?} should not be empty"
            );
        }
    }
}
