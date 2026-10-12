{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  command = import ./media-apps.nix { inherit pkgs; };
in pkgs.testers.runNixOSTest {
  name = "rsdm-media-windows";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 4096;
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.sway pkgs.obs-studio pkgs.mumble pkgs.waybar ];
    fonts.packages = [ pkgs.dejavu_fonts ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."test-waybar.json".text = builtins.toJSON { modules-left = [ "clock" ]; };
    environment.etc."rsdm-sway.conf".text = ''
      xwayland disable
      exec ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = { WLR_BACKENDS = "headless"; WLR_RENDERER = "pixman"; LIBGL_ALWAYS_SOFTWARE = "1"; QT_QPA_PLATFORM = "wayland"; };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.sway}/bin/sway -c /etc/rsdm-sway.conf > /tmp/media-app-session.log 2>&1
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
    machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=45)
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    session = control + "WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " SWAYSOCK=" + shlex.quote(values["SWAYSOCK"]) + " "
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    wm_before = machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID")

    def windows():
        pending = [json.loads(machine.succeed(session + "swaymsg -t get_tree -r"))]
        result = []
        while pending:
            node = pending.pop()
            pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
            if node.get("pid"):
                result.append(node)
        return result

    def unit_windows(unit):
        group = machine.succeed(user + "systemctl --user show " + unit + " -p ControlGroup --value").strip()
        assert group.startswith("/") and group != "/", group
        pids = {int(pid) for pid in machine.succeed("find " + shlex.quote("/sys/fs/cgroup" + group) + " -name cgroup.procs -exec cat {} +").split()}
        return [node for node in windows() if node["pid"] in pids]

    for application in ["obs", "mumble", "waybar"]:
        with subtest(application + ": actual owned and foreign clients"):
            foreign = "rsdm-foreign-" + application + ".service"
            machine.succeed(user + "systemd-run --user --collect --unit=" + foreign + " --property=ExitType=cgroup --setenv=QT_QPA_PLATFORM=wayland --setenv=WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " -- ${command} " + application + " foreign-" + application)
            machine.succeed(session + "rsdm app --shutdown-timeout 3 --on-timeout cancel -- ${command} " + application + " owned-" + application)
            app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
            if application == "waybar":
                machine.wait_until_succeeds(user + "systemctl --user is-active --quiet " + foreign + " " + app["unit"])
                for unit in [foreign, app["unit"]]:
                    machine.wait_until_succeeds("journalctl --no-pager _SYSTEMD_USER_UNIT=" + unit + " | grep 'Bar configured'")
                assert not unit_windows(app["unit"])
            else:
                for _ in range(40):
                    owned_windows = unit_windows(app["unit"])
                    foreign_windows = unit_windows(foreign)
                    if owned_windows and foreign_windows:
                        break
                    machine.sleep(1)
                if not owned_windows or not foreign_windows:
                    print(machine.succeed("journalctl --no-pager _UID=1000 -n 100"))
                assert owned_windows and foreign_windows
                assert all(node["shell"] == "xdg_shell" for node in owned_windows + foreign_windows)
                if application == "obs":
                    assert any(node["name"].startswith("OBS ") for node in owned_windows)
                else:
                    assert any(node["name"] == "Audio Tuning Wizard" for node in owned_windows)
            before = machine.succeed(user + "systemctl --user show " + foreign + " -p MainPID -p InvocationID")
            foreign_ids = {node["id"] for node in unit_windows(foreign)}

            if application == "mumble":
                code, output = machine.execute(session + "rsdm session stop")
                path = directory + "/apps/" + app["unit"] + ".toml"
            else:
                code, output = machine.execute(user + "systemctl --user stop " + app["unit"])
                path = directory + "/results/" + app["unit"] + ".toml"
                machine.wait_for_file(path)
            saved = tomllib.loads(machine.succeed("cat " + path))
            method = "term" if application == "waybar" else "window_close"
            assert saved["dispatch"]["method"] == method, saved
            assert saved["dispatch"]["state"] == "sent", saved
            assert not saved["preparation"]["escalated"], saved
            if application == "mumble":
                assert code != 0 and saved["preparation"]["outcome"] == "cancelled", saved
                owned_ids = {node["id"] for node in unit_windows(app["unit"])}
                assert owned_ids
                assert any(node["name"] == "Audio Tuning Wizard" for node in unit_windows(app["unit"]))
                machine.fail(session + "rsdm session stop")
                assert {node["id"] for node in unit_windows(app["unit"])} == owned_ids
            else:
                assert code == 0 and saved["preparation"]["outcome"] == "clean", (output, saved)

            assert machine.succeed(user + "systemctl --user show " + foreign + " -p MainPID -p InvocationID") == before
            assert {node["id"] for node in unit_windows(foreign)} == foreign_ids
            assert machine.succeed(user + "systemctl --user show " + record["compositor_unit"] + " -p MainPID -p InvocationID") == wm_before

            # End retained fixtures only after checking their shutdown outcome.
            if machine.execute(user + "systemctl --user is-active --quiet " + app["unit"])[0] == 0:
                machine.succeed(user + "systemctl --user kill --kill-whom=all --signal=KILL " + app["unit"])
            machine.wait_until_fails("test -e " + directory + "/apps/" + app["unit"] + ".toml")
            machine.succeed(user + "systemctl --user kill --kill-whom=all --signal=KILL " + foreign)
  '';
}
