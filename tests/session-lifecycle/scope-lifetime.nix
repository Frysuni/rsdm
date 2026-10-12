{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  python = pkgs.python3.withPackages (packages: [ packages.dbus-python ]);
  application = pkgs.writeShellScript "test-scope-lifetime-app" ''
    exec ${python}/bin/python3 ${./scope-lifetime-app.py}
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-scope-lifetime";
  globalTimeout = 120;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 1024;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.sway ];
    environment.etc."rsdm.toml".text = ''
      [logging]
      level = "debug"
    '';
    environment.etc."rsdm-sway.conf".text = ''
      exec ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = { WLR_BACKENDS = "headless"; WLR_RENDERER = "pixman"; };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.sway}/bin/sway -c /etc/rsdm-sway.conf > /tmp/rsdm-scope-session.log 2>&1
        touch /tmp/rsdm-scope-session-exited
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
    machine.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
    values = dict(line.split("=", 1) for line in machine.succeed(user + "systemctl --user show-environment").splitlines())
    directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
    record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
    compositor = record["compositor_unit"]
    before = machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
    machine.succeed(control + "rsdm app --shutdown-method term --shutdown-timeout 3 --on-timeout cancel -- ${application}")
    machine.wait_for_file("/run/user/1000/scope-app-ready")
    pid = int(machine.succeed("cat /run/user/1000/scope-app-ready"))
    scope = machine.succeed("cat /run/user/1000/scope-app-unit").strip()
    machine.wait_until_succeeds(user + "systemctl --user is-active --quiet " + scope + " || test -e /run/user/1000/scope-app-stopped")
    paths = machine.succeed("find " + directory + " -type f -name 'app-rsdm-*.toml'").splitlines()
    assert len(paths) == 1, paths
    app = tomllib.loads(machine.succeed("cat " + shlex.quote(paths[0])))
    assert app["original_process"]["pid"] == pid, app
    scope_group = machine.succeed(user + "systemctl --user show " + scope + " -p ControlGroup --value").strip()
    membership = machine.succeed("cat /proc/" + str(pid) + "/cgroup")
    assert "0::" + scope_group in membership.splitlines(), membership
    group = machine.succeed(user + "systemctl --user show " + app["unit"] + " -p ControlGroup --value").strip()
    assert group.startswith("/") and group != "/", group
    remaining = machine.succeed("cat " + shlex.quote("/sys/fs/cgroup" + group + "/cgroup.procs")).split()
    assert str(pid) not in remaining, remaining
    print("original service cgroup processes after migration", remaining)
    machine.sleep(3)
    if machine.execute("test ! -e /run/user/1000/scope-app-stopped")[0] != 0:
        print(machine.succeed("journalctl --no-pager _UID=1000 -n 100"))
    machine.succeed("test ! -e /run/user/1000/scope-app-stopped")
    machine.succeed("kill -0 " + str(pid))
    machine.succeed(user + "systemctl --user is-active --quiet " + app["unit"])
    machine.succeed(user + "systemctl --user stop " + app["unit"])
    machine.wait_for_file("/run/user/1000/scope-app-stopped")
    path = directory + "/results/" + app["unit"] + ".toml"
    machine.wait_for_file(path)
    saved = tomllib.loads(machine.succeed("cat " + shlex.quote(path)))
    assert saved["migrated_scope"]["unit"] == scope, saved
    assert saved["dispatch"]["method"] == "term", saved
    assert saved["preparation"]["outcome"] == "clean", saved
    assert not saved["preparation"]["escalated"], saved
    machine.wait_until_succeeds(user + "systemctl --user show " + scope + " -p ActiveState --value | grep '^inactive$'")
    assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == before
    machine.succeed(control + "rsdm session stop")
    machine.wait_for_file("/tmp/rsdm-scope-session-exited")
  '';
}
