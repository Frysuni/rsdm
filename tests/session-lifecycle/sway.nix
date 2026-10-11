{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  application = import ./window-app.nix { inherit pkgs; };
in pkgs.testers.runNixOSTest {
  name = "rsdm-sway-windows";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.sway ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."rsdm-sway.conf".text = ''
      xwayland disable
      exec ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = { WLR_BACKENDS = "headless"; WLR_RENDERER = "pixman"; };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.sway}/bin/sway -c /etc/rsdm-sway.conf > /tmp/rsdm-sway.log 2>&1
        touch /tmp/rsdm-sway-exited
      fi
    '';
  };
  testScript = ''
    import json
    import shlex
    import tomllib

    start_all()
    machine.wait_for_unit("multi-user.target")
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    control = "${controller} published alice "
    try:
        machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
    except Exception:
        print(machine.succeed("cat /tmp/rsdm-sway.log; journalctl --no-pager -n 100"))
        raise
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    assert dict(record["exported_environment"])["SWAYSOCK"] == values["SWAYSOCK"]
    compositor = record["compositor_unit"]
    compositor_state = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " SWAYSOCK=" + shlex.quote(values["SWAYSOCK"]) + " GSK_RENDERER=cairo "

    def windows():
        pending = [json.loads(machine.succeed(session + "swaymsg -t get_tree -r"))]
        result = []
        while pending:
            node = pending.pop()
            pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
            if node.get("pid"):
                result.append(node)
        return result

    with subtest("one Sway close preserves save dialogs and foreign windows across cancellation"):
        machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window --setenv=GSK_RENDERER=cairo ${application} foreign")
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${application} owned")
        machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'owned 1'")
        machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'foreign 1'")
        foreign = {window["id"] for window in windows() if window["name"].startswith("foreign ")}
        assert len(foreign) == 2
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["name"].startswith("owned ")]) == 3
        assert foreign == {window["id"] for window in windows() if window["name"].startswith("foreign ")}
        machine.fail("test -e /run/user/1000/owned-term")
        machine.fail("test -e /run/user/1000/foreign-closes")
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        application_record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        assert application_record["dispatch"]["method"] == "window_close"
        assert application_record["dispatch"]["state"] == "sent"
        assert application_record["preparation"]["outcome"] == "cancelled"
        assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == compositor_state
        machine.succeed("touch /run/user/1000/owned-finish /run/user/1000/foreign-finish")
        machine.wait_until_fails("test -e " + directory + "/apps/" + application_record["unit"] + ".toml")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-foreign-window")

    with subtest("an individual stop records clean exit after one Sway close"):
        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${application} clean")
        machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'clean 1'")
        application_record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.succeed(user + "systemctl --user stop " + application_record["unit"])
        machine.wait_until_succeeds("test -e " + directory + "/results/" + application_record["unit"] + ".toml")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + application_record["unit"] + ".toml"))
        assert saved["dispatch"]["method"] == "window_close"
        assert saved["preparation"]["outcome"] == "clean"
        assert not saved["preparation"]["escalated"]
        assert machine.succeed("cat /run/user/1000/clean-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/clean-term")
        assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == compositor_state
        machine.succeed(session + "rsdm session stop")
        machine.wait_for_file("/tmp/rsdm-sway-exited")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet " + compositor)
  '';
}
