{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  application = import ./window-app.nix { inherit pkgs; };
  xapplication = import ./window-app.nix { inherit pkgs; backend = "x11"; };
  session = pkgs.writeShellScript "test-kwin-session" ''
    ${rsdm}/bin/rsdm session finalize
    exec ${pkgs.coreutils}/bin/sleep infinity
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-kwin-windows";
  globalTimeout = 180;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.kdePackages.kwin pkgs.xwayland pkgs.xorg.xprop ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."xdg/menus/applications.menu".text = ''
      <!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN" "http://www.freedesktop.org/standards/menu-spec/1.0/menu.dtd">
      <Menu><Name>Applications</Name><DefaultAppDirs/><Include><All/></Include></Menu>
    '';
    environment.variables = {
      LIBGL_ALWAYS_SOFTWARE = "1";
      KWIN_COMPOSE = "Q";
      QT_QPA_PLATFORM = "offscreen";
      QT_LOGGING_RULES = "kwin_core.debug=true;KWIN_UTILS.debug=true";
    };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.kdePackages.kwin}/bin/kwin_wayland --virtual --no-lockscreen --xwayland --exit-with-session ${session} > /tmp/rsdm-kwin.log 2>&1
        touch /tmp/rsdm-kwin-exited
      fi
    '';
  };
  testScript = ''
    import shlex
    import tomllib

    start_all()
    machine.wait_for_unit("multi-user.target")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    control = "${controller} published alice "
    try:
        machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
    except Exception:
        print(machine.succeed("cat /tmp/rsdm-kwin.log; journalctl --no-pager -n 100"))
        raise
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    compositor = record["compositor_unit"]
    before = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " DISPLAY=" + shlex.quote(values["DISPLAY"]) + " GSK_RENDERER=cairo "
    status = machine.succeed(session + "rsdm session status")
    if "window-close: available" not in status:
        print(machine.succeed("cat /tmp/rsdm-kwin.log; cat /run/current-system/sw/share/applications/org.rsdm.SessionManager.desktop; journalctl --no-pager _UID=1000 -n 100"))
    assert "window-close: available (WAYLAND_DISPLAY: authenticated KWin" in status, status
    machine.succeed("test -e /run/current-system/sw/share/applications/org.rsdm.SessionManager.desktop")
    pid = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID --value").strip()
    assert "KWIN_WAYLAND_NO_PERMISSION_CHECKS=1" not in machine.succeed("cat /proc/" + pid + "/environ")

    for label, command in [("owned", "${application}"), ("xowned", "${xapplication}")]:
        with subtest(label + ": one live-handle close preserves dialogs and foreign windows"):
            foreign = "foreign" + label
            machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window --setenv=GSK_RENDERER=cairo " + command + " " + foreign)
            machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- " + command + " " + label)
            machine.wait_for_file("/run/user/1000/" + label + "-ready")
            machine.wait_for_file("/run/user/1000/" + foreign + "-ready")
            app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
            if label == "xowned":
                pid = machine.succeed(user + "systemctl --user show " + app["unit"] + " -p MainPID --value").strip()
                machine.succeed(session + "xprop -name '" + foreign + " 1' -f _NET_WM_PID 32c -set _NET_WM_PID " + pid)
            machine.fail(session + "rsdm session stop")
            assert machine.succeed("cat /run/user/1000/" + label + "-closes").splitlines() == ["close"]
            assert machine.succeed("cat /run/user/1000/" + label + "-windows").strip() == "3"
            assert machine.succeed("cat /run/user/1000/" + foreign + "-windows").strip() == "2"
            machine.fail("test -e /run/user/1000/" + label + "-term")
            machine.fail("test -e /run/user/1000/" + foreign + "-closes")
            machine.succeed(user + "systemctl --user is-active --quiet rsdm-foreign-window")
            machine.fail(session + "rsdm session stop")
            assert machine.succeed("cat /run/user/1000/" + label + "-closes").splitlines() == ["close"]
            saved = tomllib.loads(machine.succeed("cat " + directory + "/apps/" + app["unit"] + ".toml"))
            assert saved["dispatch"]["method"] == "window_close"
            assert saved["dispatch"]["state"] == "sent"
            assert saved["preparation"]["outcome"] == "cancelled"
            assert not saved["preparation"]["escalated"]
            assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == before
            machine.succeed("touch /run/user/1000/" + label + "-finish /run/user/1000/" + foreign + "-finish")
            machine.wait_until_fails("test -e " + directory + "/apps/" + app["unit"] + ".toml")
            machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-foreign-window")

    with subtest("clean exit follows one close without escalation"):
        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${application} clean")
        machine.wait_for_file("/run/user/1000/clean-ready")
        app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.succeed(user + "systemctl --user stop " + app["unit"])
        machine.wait_for_file(directory + "/results/" + app["unit"] + ".toml")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + app["unit"] + ".toml"))
        assert saved["dispatch"]["method"] == "window_close"
        assert saved["dispatch"]["state"] == "sent"
        assert saved["preparation"]["outcome"] == "clean"
        assert not saved["preparation"]["escalated"]
        assert machine.succeed("cat /run/user/1000/clean-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/clean-term")
        assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == before
        machine.succeed(session + "rsdm session stop")
        machine.wait_for_file("/tmp/rsdm-kwin-exited")
  '';
}
