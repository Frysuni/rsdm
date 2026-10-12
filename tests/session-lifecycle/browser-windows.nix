{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  browsers = import ./browser-apps.nix { inherit pkgs; };
in pkgs.testers.runNixOSTest {
  name = "rsdm-browser-windows";
  globalTimeout = 300;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 4096;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.sway pkgs.firefox pkgs.chromium pkgs.xwayland ];
    fonts.packages = [ pkgs.dejavu_fonts ];
    environment.etc."rsdm.toml".text = ''
      [logging]
      level = "debug"
    '';
    environment.etc."rsdm-browser.html".text = "<!doctype html><title>RSDM browser matrix</title><p>Local session test</p>";
    environment.etc."rsdm-sway.conf".text = ''
      exec ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = { WLR_BACKENDS = "headless"; WLR_RENDERER = "pixman"; LIBGL_ALWAYS_SOFTWARE = "1"; };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.sway}/bin/sway -c /etc/rsdm-sway.conf > /tmp/rsdm-browser-session.log 2>&1
        touch /tmp/rsdm-browser-session-exited
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
    machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    compositor = record["compositor_unit"]
    before = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " SWAYSOCK=" + shlex.quote(values["SWAYSOCK"]) + " DISPLAY=" + shlex.quote(values["DISPLAY"]) + " "
    assert "window-close: available (SWAYSOCK:" in machine.succeed(session + "rsdm session status")

    def windows():
        pending = [json.loads(machine.succeed(session + "swaymsg -t get_tree -r"))]
        result = []
        while pending:
            node = pending.pop()
            pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
            if node.get("pid"):
                result.append(node)
        return result

    def group_processes(unit):
        group = machine.succeed(user + "systemctl --user show " + unit + " -p ControlGroup --value").strip()
        assert group.startswith("/") and group != "/", group
        command = "find " + shlex.quote("/sys/fs/cgroup" + group) + " -name cgroup.procs -exec cat {} +"
        return {int(pid) for pid in machine.succeed(command).split()}

    def migrated_scope(unit):
        main = int(machine.succeed(user + "systemctl --user show " + unit + " -p MainPID --value"))
        if not main:
            return None
        group = next(line[3:] for line in machine.succeed("cat /proc/" + str(main) + "/cgroup").splitlines() if line.startswith("0::"))
        service_group = machine.succeed(user + "systemctl --user show " + unit + " -p ControlGroup --value").strip()
        if group == service_group or group.startswith(service_group + "/"):
            return None
        scope = group.rsplit("/", 1)[1]
        assert scope.endswith(".scope"), (unit, main, group)
        assert machine.succeed(user + "systemctl --user show " + scope + " -p ControlGroup --value").strip() == group
        if unit.startswith("app-rsdm-"):
            app = tomllib.loads(machine.succeed("cat " + shlex.quote(directory + "/apps/" + unit + ".toml")))
            captured = app.get("migrated_scope")
            if not captured:
                return None
            assert captured["unit"] == scope, app
            assert bytes(captured["invocation_id"]).hex() == machine.succeed(user + "systemctl --user show " + scope + " -p InvocationID --value").strip()
            assert app["original_process"]["pid"] == main, app
            identity = machine.succeed("${pkgs.python3}/bin/python3 -c 'import os; fd = os.pidfd_open(" + str(main) + "); print(os.fstat(fd).st_ino)'")
            assert app["original_process"]["pidfd_id"] == int(identity), app
        return scope

    def processes(unit):
        result = group_processes(unit)
        scope = migrated_scope(unit)
        if scope:
            result |= group_processes(scope)
        return result

    def unit_windows(unit):
        pids = processes(unit)
        return [window for window in windows() if window["pid"] in pids]

    cases = [("chromium", "${browsers.chromium}"), ("firefox", "${browsers.firefox}")]
    for browser, command in cases:
        for backend in ["wayland", "x11"]:
            label = browser + "-" + backend
            with subtest(label + ": registered browser exits while foreign browser stays alive"):
                foreign = "rsdm-foreign-" + label + ".service"
                machine.succeed(user + "systemd-run --user --collect --unit=" + foreign + " --setenv=WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " --setenv=DISPLAY=" + shlex.quote(values["DISPLAY"]) + " " + command + " foreign-" + label + " " + backend)
                machine.succeed(session + "rsdm app --shutdown-timeout 10 --on-timeout cancel -- " + command + " owned-" + label + " " + backend)
                app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
                machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'RSDM browser matrix'", timeout=45)
                for unit in [foreign, app["unit"]]:
                    machine.wait_until_succeeds(user + "systemctl --user is-active --quiet " + unit)
                for _ in range(45):
                    owned_windows = unit_windows(app["unit"])
                    foreign_windows = unit_windows(foreign)
                    if owned_windows and foreign_windows:
                        break
                    machine.sleep(1)
                if not owned_windows or not foreign_windows:
                    print(label, "windows", windows())
                    print("owned processes", processes(app["unit"]), "foreign processes", processes(foreign))
                    for window in windows():
                        print("window process", window["pid"], machine.execute("cat /proc/" + str(window["pid"]) + "/cgroup"))
                    print(machine.succeed(user + "systemctl --user show " + app["unit"] + " -p MainPID -p ControlGroup -p InvocationID"))
                    print(machine.succeed("journalctl --no-pager _UID=1000 -n 60"))
                assert len(owned_windows) == len(foreign_windows) == 1, (label, owned_windows, foreign_windows)
                shell = "xdg_shell" if backend == "wayland" else "xwayland"
                assert owned_windows[0]["shell"] == foreign_windows[0]["shell"] == shell
                foreign_ids = {window["id"] for window in foreign_windows}
                foreign_state = machine.succeed(user + "systemctl --user show " + foreign + " -p MainPID -p InvocationID")
                foreign_scope = migrated_scope(foreign)
                foreign_scope_state = machine.succeed(user + "systemctl --user show " + foreign_scope + " -p InvocationID") if foreign_scope else None
                own_scope = migrated_scope(app["unit"])
                scope_group = machine.succeed(user + "systemctl --user show " + own_scope + " -p ControlGroup --value").strip() if own_scope else None
                app_group = machine.succeed(user + "systemctl --user show " + app["unit"] + " -p ControlGroup --value").strip()

                machine.succeed(user + "systemctl --user stop " + app["unit"])
                path = directory + "/results/" + app["unit"] + ".toml"
                machine.wait_for_file(path)
                saved = tomllib.loads(machine.succeed("cat " + shlex.quote(path)))
                if backend == "wayland":
                    if saved["dispatch"]["method"] != "window_close":
                        print(machine.succeed("journalctl --no-pager _UID=1000 -n 100"))
                    assert saved["dispatch"]["method"] == "window_close", saved
                    assert saved["dispatch"]["state"] == "sent", saved
                    assert saved["preparation"]["outcome"] == "clean", saved
                    assert not saved["preparation"]["escalated"], saved
                else:
                    assert saved["dispatch"]["method"] in ["term", "xsmp"], saved
                    assert saved["preparation"]["outcome"] == "clean", saved
                    assert not saved["preparation"]["escalated"], saved
                    assert not saved.get("window_close_sent", False), saved
                machine.fail(user + "systemctl --user is-active --quiet " + app["unit"])
                machine.succeed("test ! -d " + shlex.quote("/sys/fs/cgroup" + app_group))
                if own_scope:
                    assert scope_group is not None
                    machine.fail(user + "systemctl --user is-active --quiet " + own_scope)
                    machine.succeed("test ! -d " + shlex.quote("/sys/fs/cgroup" + scope_group))
                assert not any(window["id"] in {owned["id"] for owned in owned_windows} for window in windows())
                assert {window["id"] for window in unit_windows(foreign)} == foreign_ids
                assert machine.succeed(user + "systemctl --user show " + foreign + " -p MainPID -p InvocationID") == foreign_state
                if foreign_scope:
                    assert machine.succeed(user + "systemctl --user show " + foreign_scope + " -p InvocationID") == foreign_scope_state
                assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == before
                print(label, saved["preparation"])
                machine.succeed(user + "systemctl --user stop " + " ".join([foreign] + ([foreign_scope] if foreign_scope else [])))

    machine.succeed(session + "rsdm session stop")
    machine.wait_for_file("/tmp/rsdm-browser-session-exited")
  '';
}
