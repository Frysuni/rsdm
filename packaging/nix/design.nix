{ lib }:
let
  # Enum rosters, kept in lock-step with the serde names in rsdm-core. Adding a
  # theme/border/background/preset is a one-line change here and in the Rust enum.
  themeNames = [
    "minimal"
    "monochrome"
    "cyberpunk"
    "catppuccin"
    "gruvbox"
    "nord"
    "dracula"
    "tokyo-night"
    "material"
    "solarized"
    "eldritch"
    "rama"
    "dark"
    "trans-is-hard-job"
  ];
  borderStyles = [
    "none"
    "classic"
    "modern"
    "minimal"
    "ascii1"
    "ascii2"
    "ascii3"
    "ascii4"
    "wave"
    "pulse"
  ];
  backgrounds = [
    "none"
    "matrix"
    "fire"
    "rain"
    "plasma"
    "starfield"
  ];
  titleModes = [
    "custom"
    "hostname"
    "os-release"
    "session-name"
    "preset-logo"
  ];
  logoPresets = [
    "minimal"
    "block"
    "rsdm"
    "rustty"
    "waytty"
    "niri"
    "hyprland"
    "kde"
    "gnome"
    "nixos"
    "arch"
    "tux"
  ];
  # The design options (DesignConfig) shared in shape by both fronts. `dm.design`
  # and `lock.design` each instantiate this independently - no shared defaults,
  # no overrides, just two complete designs. The wallpaper fields exist only on
  # the locker (the framebuffer front that can paint an image); the greeter has
  # no wallpaper, so its design submodule omits them entirely.
  mkDesignOptions =
    { wallpaper }:
    {
      theme = lib.mkOption {
        type = lib.types.enum themeNames;
        default = "monochrome";
        description = "Color theme for this front.";
      };
      borderStyle = lib.mkOption {
        type = lib.types.enum borderStyles;
        default = "classic";
        description = ''
          Frame style for the login box / lock card. classic = double frame +
          inner panel; modern = single frame; none = no frame or panel;
          minimal = panel without frame; ascii1 = solid
          block; ascii2 = gradient fade; ascii3 = layered panel; ascii4 = block
          banners; wave / pulse = variants.
        '';
      };
      background = lib.mkOption {
        type = lib.types.enum backgrounds;
        default = "none";
        description = ''
          Animated backdrop. "none" stays still (and lets the locker show its
          wallpaper). An animated choice drives a continuous redraw.
        '';
      };
      backgroundSpeed = lib.mkOption {
        type = lib.types.ints.between 0 10;
        default = 5;
        description = "Background animation speed, 0 (frozen) to 10 (fast); 5 is the natural rate.";
      };
      titleMode = lib.mkOption {
        type = lib.types.enum titleModes;
        default = "custom";
        description = "What the centered banner shows.";
      };
      titleText = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Banner text for titleMode = custom. null falls back to RSDM.";
      };
      titlePreset = lib.mkOption {
        type = lib.types.enum logoPresets;
        default = "rsdm";
        description = "Logo for titleMode = preset-logo.";
      };
      titleFont = lib.mkOption {
        type = lib.types.str;
        default = "ANSI Shadow";
        description = ''
          figrs/FIGlet font name for the generated ASCII-art banner. The runtime
          F1 menu shows the full scrollable figrs font list, so this is a string
          rather than a fixed enum.
        '';
      };
      passwordMode = lib.mkOption {
        type = lib.types.enum [
          "hidden"
          "asterisks"
        ];
        default = "asterisks";
        description = "How the typed password is shown.";
      };
      menu = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether the runtime design switcher (F1) is available on this front.
          Changes made through it apply live but are never persisted - every
          launch starts from this config.
        '';
      };
      showClock = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Show the clock.";
      };
      showHostname = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Show the hostname in the top status line.";
      };
    }
    // lib.optionalAttrs wallpaper {
      wallpaper = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Background image (PNG/JPEG) used as the lock screen base layer.";
      };
      wallpaperDim = lib.mkOption {
        type = lib.types.ints.between 0 10;
        default = 6;
        description = ''
          How dark the wallpaper is drawn: 0 blacks it out entirely, 10 leaves it
          untouched. Also switchable at runtime through F1 -> Wallpaper dim.
        '';
      };
      backgroundOpacity = lib.mkOption {
        type = lib.types.ints.between 0 10;
        default = 8;
        description = ''
          Opacity of the animated background blended over the wallpaper: 0 shows
          only the wallpaper, 10 draws the animation fully opaque. Ignored without
          a wallpaper.
        '';
      };
    };

  # Render a design submodule to the TOML `design` table. Scalars are always
  # emitted; the nullable fields (title_text, wallpaper) are dropped when unset
  # because `pkgs.formats.toml` has no representation for null. The wallpaper
  # fields only exist on the locker design, guarded by `d ? wallpaper`.
  designToToml =
    d:
    {
      theme = d.theme;
      border_style = d.borderStyle;
      background = d.background;
      background_speed = d.backgroundSpeed;
      title_mode = d.titleMode;
      title_preset = d.titlePreset;
      title_font = d.titleFont;
      password_mode = d.passwordMode;
      menu = d.menu;
      show_clock = d.showClock;
      show_hostname = d.showHostname;
    }
    // lib.optionalAttrs (d.titleText != null) { title_text = d.titleText; }
    // lib.optionalAttrs (d ? wallpaper) {
      wallpaper_dim = d.wallpaperDim;
      background_opacity = d.backgroundOpacity;
    }
    // lib.optionalAttrs (d ? wallpaper && d.wallpaper != null) {
      wallpaper = toString d.wallpaper;
    };
in
{
  inherit designToToml mkDesignOptions;
}
