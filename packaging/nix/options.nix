{
  defaultPackage,
  lib,
  mkDesignOptions,
  pkgs,
  toml,
  ttyName,
}:
{
  enable = lib.mkEnableOption "rsdm display manager";

  package = lib.mkOption {
    type = lib.types.package;
    default = defaultPackage;
    defaultText = lib.literalExpression "inputs.rsdm.packages.${pkgs.system}.default";
  };

  sessionManager = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "Wrap the compositor in the systemd --user graphical session manager (uwsm-style).";
  };

  keyring = lib.mkOption {
    type = lib.types.enum [
      "auto"
      "gnome"
      "kwallet"
      "none"
    ];
    default = "auto";
    description = ''
      Which keyring module the greeter PAM stack (`rsdm`) runs, so the keyring
      is unlocked by the same login that authenticates you - exactly as login,
      gdm and sddm do it. "auto" mirrors the system: gnome-keyring when ANY PAM
      service enables it (login, greetd, sddm, gdm, a custom service, ...) or
      services.gnome.gnome-keyring.enable is set; kwallet when any PAM service
      enables it. "gnome"/"kwallet" force one, "none" runs no keyring module.
      For a fully custom or non-standard keyring, leave this at "none" and add
      your own module via security.pam.services.rsdm.rules (or .text).
    '';
  };

  disableGetty = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "Disable getty on the selected virtual terminal.";
  };

  displayManagerAlias = lib.mkOption {
    type = lib.types.nullOr lib.types.bool;
    default = null;
    description = ''
      Whether to install the display-manager.service alias. The automatic
      default enables it only when services.rsdm.dm.tty is tty1, so a
      non-primary TTY service does not replace an existing display manager role.
    '';
  };

  logging.level = lib.mkOption {
    type = lib.types.enum [
      "error"
      "warn"
      "info"
      "debug"
      "trace"
    ];
    default = "info";
    description = "rsdm log verbosity. RUST_LOG can still override this for one-off debugging.";
  };

  logging.file = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = ''
      Optional extra log file. rsdm always logs to the journal (stderr, routed
      by systemd); when this is set the same records are additionally teed into
      this absolute path. null keeps the journal as the only sink.
    '';
  };

  dm = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Run the greeter. When false, `rsdm dm` exits without presenting a login.";
    };

    tty = lib.mkOption {
      type = lib.types.str;
      default = "tty1";
      description = "Linux virtual terminal the greeter owns, such as tty1 or /dev/tty2.";
    };

    seat = lib.mkOption {
      type = lib.types.str;
      default = "seat0";
      description = "logind seat the greeter and launched session belong to.";
    };

    fixedSession = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        A `.desktop` id, a session Name, or a literal command to always launch,
        hiding the session picker (for example "niri-session"). null shows the
        picker over discovered Wayland sessions.
      '';
    };

    sessionDirs = lib.mkOption {
      type = lib.types.nullOr (lib.types.listOf lib.types.str);
      default = null;
      description = ''
        Directories scanned for wayland-sessions `.desktop` files when no fixed
        session is set. null uses the built-in defaults.
      '';
    };

    fallback.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Hand the TTY to a plain login when the greeter cannot run.";
    };

    fallback.command = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = ''
        Argv of the fallback login program, exec'd in place of the greeter when
        it cannot run or the user asks to exit to the console. Leave empty (the
        default) to auto-resolve: rsdm asks systemd what `getty@${ttyName}.service`
        runs and reuses that console login verbatim (`agetty` on most distros,
        but `mingetty`/`busybox getty`/an unusual path elsewhere), then falls
        back to built-in `agetty`/`login` candidates. Set an explicit argv only
        to override that. agetty is preferred over a bare `login`: it opens and
        configures the VT itself and then runs `login`, whereas `login` exits
        immediately when it does not own the terminal and just bounces back into
        the greeter. The greeter returns after the console session ends (the
        service restarts).
      '';
    };

    remember.username = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Pre-fill the last successful username on the next launch.";
    };

    remember.session = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Pre-select the last used session on the next launch (picker mode only).";
    };

    design = lib.mkOption {
      type = lib.types.submodule { options = mkDesignOptions { wallpaper = false; }; };
      default = { };
      description = "The greeter's look (a DesignConfig), configured independently of lock.design.";
    };
  };

  lock = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Whether locking is possible at all. When false, `rsdm lock` refuses to
        lock the screen, so an idle daemon can be wired up without ever blanking.
      '';
    };

    primaryOutput = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Wayland output that shows the interactive unlock UI (for example
        "DP-1"). null selects the connected output with the largest current
        pixel area. Find names with `niri msg outputs` or the compositor's
        equivalent output-inspection command.
      '';
    };

    secondaryOutput = lib.mkOption {
      type = lib.types.enum [
        "background"
        "black"
        "off"
      ];
      default = "background";
      description = ''
        What secondary outputs show while locked. background keeps only the
        wallpaper/effect, black paints opaque black, and off temporarily powers
        them down through supported compositor IPC (currently niri; otherwise
        it safely falls back to black).
      '';
    };

    size = lib.mkOption {
      type = lib.types.nullOr (lib.types.ints.between 1 12);
      default = null;
      description = ''
        Integer bitmap-glyph zoom for the lock UI. null automatically targets
        a TTY-like text density (about 68 rows); 1 through 12 force an exact
        crisp pixel zoom. Also adjustable at runtime through F1.
      '';
    };

    design = lib.mkOption {
      type = lib.types.submodule { options = mkDesignOptions { wallpaper = true; }; };
      default = { };
      description = ''
        The lock screen's look (a DesignConfig, plus the locker-only wallpaper
        fields), configured independently of dm.design.
      '';
    };
  };

  idle = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Run rsdm's compositor-driven idle monitor in graphical user sessions.";
    };

    timeout = lib.mkOption {
      type = lib.types.ints.between 1 4294967;
      default = 300;
      description = "Seconds of inactivity before rsdm starts the lock screen.";
    };

    ignoreInhibitors = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Ignore idle inhibitors and track raw keyboard/pointer activity only.";
    };

    lockCommand = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Optional locker argv. Empty uses rsdm lock with a confirmed-lock handshake.";
    };

    onLock = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Shell commands run after the idle lock is confirmed active.";
    };

    onUnlock = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Shell commands run after the idle-started locker exits.";
    };
  };

  extraConfig = lib.mkOption {
    type = toml.type;
    default = { };
    description = "Additional rsdm.toml values merged last.";
  };

  config = lib.mkOption {
    type = toml.type;
    default = { };
    description = "Deprecated alias merged before extraConfig.";
  };
}
