{ pkgs }:

{
  firefox = pkgs.writeShellScript "test-firefox-window" ''
    set -eu
    profile="$HOME/$1"
    mkdir -p "$profile"
    cat > "$profile/user.js" <<'PREFS'
    user_pref("browser.shell.checkDefaultBrowser", false);
    user_pref("browser.aboutwelcome.enabled", false);
    user_pref("browser.startup.homepage_override.mstone", "ignore");
    user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
    PREFS
    export GDK_BACKEND="$2"
    if [ "$2" = wayland ]; then
      export MOZ_ENABLE_WAYLAND=1
    else
      export MOZ_ENABLE_WAYLAND=0
    fi
    exec ${pkgs.firefox}/bin/firefox --no-remote --profile "$profile" file:///etc/rsdm-browser.html
  '';
  chromium = pkgs.writeShellScript "test-chromium-window" ''
    set -eu
    exec ${pkgs.chromium}/bin/chromium --user-data-dir="$HOME/$1" \
      --ozone-platform="$2" --no-first-run --no-default-browser-check \
      --disable-dev-shm-usage --disable-gpu file:///etc/rsdm-browser.html
  '';
}
