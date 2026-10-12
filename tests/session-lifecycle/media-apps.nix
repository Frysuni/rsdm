{ pkgs }:

pkgs.writeShellScript "test-media-app" ''
  set -eu
  application="$1"
  profile="$2"
  export XDG_CONFIG_HOME="$HOME/$profile"
  mkdir -p "$XDG_CONFIG_HOME"
  case "$application" in
    obs)
      mkdir -p "$XDG_CONFIG_HOME/obs-studio"
      printf '[General]\nFirstRun=true\n' > "$XDG_CONFIG_HOME/obs-studio/user.ini"
      exec ${pkgs.obs-studio}/bin/obs --multi --disable-missing-files-check
      ;;
    mumble)
      exec ${pkgs.mumble}/bin/mumble --multiple --no-identity
      ;;
    waybar)
      exec ${pkgs.waybar}/bin/waybar --config /etc/test-waybar.json
      ;;
  esac
  ''
