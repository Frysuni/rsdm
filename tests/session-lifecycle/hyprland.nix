{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  application = import ./window-app.nix { inherit pkgs; };
in pkgs.testers.runNixOSTest {
  name = "rsdm-hyprland-windows";
  globalTimeout = 150;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.hyprland ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."rsdm-hyprland.lua".text = ''
      hl.monitor({ output = "", mode = "preferred", position = "auto", scale = 1 })
      hl.config({ animations = { enabled = false }, xwayland = { enabled = false } })
      hl.on("hyprland.start", function()
        hl.exec_cmd("${rsdm}/bin/rsdm session finalize")
      end)
    '';
    environment.variables.LIBGL_ALWAYS_SOFTWARE = "1";
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.hyprland}/bin/Hyprland --config /etc/rsdm-hyprland.lua > /tmp/rsdm-hyprland.log 2>&1
        touch /tmp/rsdm-hyprland-exited
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
        print(machine.succeed("cat /tmp/rsdm-hyprland.log; journalctl --no-pager -n 100"))
        raise
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    assert dict(record["exported_environment"])["HYPRLAND_INSTANCE_SIGNATURE"] == values["HYPRLAND_INSTANCE_SIGNATURE"]
    compositor = record["compositor_unit"]
    compositor_state = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " HYPRLAND_INSTANCE_SIGNATURE=" + shlex.quote(values["HYPRLAND_INSTANCE_SIGNATURE"]) + " GSK_RENDERER=cairo "

    def windows():
        return json.loads(machine.succeed(session + "hyprctl -j clients"))

    with subtest("atomic stable-ID close preserves save dialogs and foreign windows"):
        machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window --setenv=GSK_RENDERER=cairo ${application} foreign")
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${application} owned")
        machine.wait_until_succeeds(session + "hyprctl -j clients | grep 'owned 1'")
        machine.wait_until_succeeds(session + "hyprctl -j clients | grep 'foreign 1'")
        foreign = {window["stableId"] for window in windows() if window["title"].startswith("foreign ")}
        assert len(foreign) == 2
        assert len({window["stableId"] for window in windows()}) == 4
        try:
            machine.fail(session + "rsdm session stop")
        except Exception:
            print(machine.execute("ls /run/user/1000/*-closes /run/user/1000/*-term")[1])
            print(machine.execute("cat /tmp/rsdm-hyprland.log")[1])
            print(machine.execute("journalctl --no-pager -n 80")[1])
            raise
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["title"].startswith("owned ")]) == 3
        assert foreign == {window["stableId"] for window in windows() if window["title"].startswith("foreign ")}
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

    with subtest("individual stop observes a clean exit after one native close"):
        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${application} clean")
        machine.wait_until_succeeds(session + "hyprctl -j clients | grep 'clean 1'")
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
        machine.wait_for_file("/tmp/rsdm-hyprland-exited")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet " + compositor)
  '';
}
