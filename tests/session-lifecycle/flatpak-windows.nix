{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  install = import ./flatpak-app.nix { inherit pkgs; };
in pkgs.testers.runNixOSTest {
  name = "rsdm-flatpak-windows";
  globalTimeout = 150;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    services.flatpak.enable = true;
    xdg.portal.enable = true;
    xdg.portal.extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
    environment.systemPackages = [ rsdm pkgs.sway pkgs.flatpak ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."rsdm-sway.conf".text = ''
      xwayland disable
      exec ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = { WLR_BACKENDS = "headless"; WLR_RENDERER = "pixman"; LIBGL_ALWAYS_SOFTWARE = "1"; };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.sway}/bin/sway -c /etc/rsdm-sway.conf > /tmp/rsdm-flatpak-session.log 2>&1
        touch /tmp/rsdm-flatpak-session-exited
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
    compositor = record["compositor_unit"]
    before = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    machine.succeed(user + "${install}")

    def windows():
        pending = [json.loads(machine.succeed(session + "swaymsg -t get_tree -r"))]
        result = []
        while pending:
            node = pending.pop()
            pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
            if node.get("pid"):
                result.append(node)
        return result

    def scope_for_process(pid):
        group = next(line[3:] for line in machine.succeed("cat /proc/" + str(pid) + "/cgroup").splitlines() if line.startswith("0::"))
        scope = group.rsplit("/", 1)[1]
        assert scope.endswith(".scope"), (pid, group)
        assert machine.succeed(user + "systemctl --user show " + scope + " -p ControlGroup --value").strip() == group
        return scope

    def own_windows(scope):
        group = machine.succeed(user + "systemctl --user show " + scope + " -p ControlGroup --value").strip()
        assert group.startswith("/") and group != "/", group
        pids = {int(pid) for pid in machine.succeed("find " + shlex.quote("/sys/fs/cgroup" + group) + " -name cgroup.procs -exec cat {} +").split()}
        return [window for window in windows() if window["pid"] in pids]

    machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-flatpak --setenv=WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " -- flatpak run --user org.rsdm.WindowTest foreign")
    machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'foreign 1'", timeout=30)
    foreign_windows = [window for window in windows() if window["name"].startswith("foreign ")]
    assert len(foreign_windows) == 2
    foreign_ids = {window["id"] for window in foreign_windows}
    foreign_scope = scope_for_process(foreign_windows[0]["pid"])
    foreign_state = machine.succeed(user + "systemctl --user show " + foreign_scope + " -p InvocationID -p ControlGroup")

    for prefix in ["owned", "clean"]:
        with subtest(prefix + ": Flatpak window ownership excludes a foreign instance of the same app"):
            machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- flatpak run --user org.rsdm.WindowTest " + prefix)
            machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep '" + prefix + " 1'", timeout=30)
            app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
            scope = scope_for_process(app["original_process"]["pid"])
            assert scope != foreign_scope
            initial = own_windows(scope)
            assert len(initial) == 2
            assert all(window["shell"] == "xdg_shell" and window["app_id"] == foreign_windows[0]["app_id"] for window in initial)
            if prefix == "owned":
                machine.fail(session + "rsdm session stop")
                saved = tomllib.loads(machine.succeed("cat " + directory + "/apps/" + app["unit"] + ".toml"))
                assert saved["preparation"]["outcome"] == "cancelled", saved
                ids = {window["id"] for window in own_windows(scope)}
                assert len(ids) == 3
                assert any(window["name"] == "owned save" for window in own_windows(scope))
                machine.fail(session + "rsdm session stop")
                assert {window["id"] for window in own_windows(scope)} == ids
            else:
                machine.succeed(user + "systemctl --user stop " + app["unit"])
                machine.wait_for_file(directory + "/results/" + app["unit"] + ".toml")
                saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + app["unit"] + ".toml"))
                assert saved["preparation"]["outcome"] == "clean", saved
                machine.fail(user + "systemctl --user is-active --quiet " + scope)
            assert saved["dispatch"]["method"] == "window_close", saved
            assert saved["dispatch"]["state"] == "sent", saved
            assert not saved["preparation"]["escalated"], saved
            assert saved["migrated_scope"]["unit"] == scope, saved
            assert saved["migrated_scope"]["lineage"], saved
            assert saved["original_process"] in saved["migrated_scope"]["lineage"], saved
            assert {window["id"] for window in own_windows(foreign_scope)} == foreign_ids
            assert machine.succeed(user + "systemctl --user show " + foreign_scope + " -p InvocationID -p ControlGroup") == foreign_state
            assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == before
            if prefix == "owned":
                # End the fixture only after checking the preserved save dialog.
                machine.succeed(user + "systemctl --user kill --kill-whom=all --signal=KILL " + scope)
                machine.wait_until_fails("test -e " + directory + "/apps/" + app["unit"] + ".toml")

    machine.succeed(user + "systemctl --user kill --kill-whom=all --signal=KILL " + foreign_scope)
    machine.succeed(session + "rsdm session stop")
    machine.wait_for_file("/tmp/rsdm-flatpak-session-exited")
  '';
}
