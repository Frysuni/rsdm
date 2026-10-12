{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  application = import ./window-app.nix { inherit pkgs; };
  node = plugins: { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.wayfire pkgs.python3 ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."rsdm-wayfire.ini".text = ''
      [core]
      plugins = ${plugins}
      xwayland = false
      [autostart]
      rsdm = ${rsdm}/bin/rsdm session finalize
    '';
    environment.variables = {
      WLR_BACKENDS = "headless";
      WLR_RENDERER = "gles2";
      WLR_RENDERER_ALLOW_SOFTWARE = "1";
      LIBGL_ALWAYS_SOFTWARE = "1";
    };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${pkgs.wayfire}/bin/wayfire -c /etc/rsdm-wayfire.ini > /tmp/rsdm-wayfire.log 2>&1
        touch /tmp/rsdm-wayfire-exited
      fi
    '';
  };
  query = pkgs.writeText "wayfire-read-views.py" ''
    import json
    import os
    import socket
    import struct

    def read_exact(peer, size):
        result = b""
        while len(result) < size:
            part = peer.recv(size - len(result))
            if not part:
                raise RuntimeError("incomplete Wayfire response")
            result += part
        return result

    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(2)
        peer.connect(os.environ["WAYFIRE_SOCKET"])
        payload = json.dumps({"method": "window-rules/list-views", "data": {}}).encode()
        peer.sendall(struct.pack("=I", len(payload)) + payload)
        length = struct.unpack("=I", read_exact(peer, 4))[0]
        assert length <= 1048576
        print(read_exact(peer, length).decode())
  '';
in pkgs.testers.runNixOSTest {
  name = "rsdm-wayfire-discovery";
  globalTimeout = 180;
  nodes.machine = node "ipc ipc-rules autostart";
  nodes.missing = node "ipc autostart";
  testScript = ''
    import json
    import shlex
    import tomllib

    start_all()
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    control = "${controller} published alice "
    for vm, reason in [(machine, "discovery available"), (missing, "ipc-rules is unavailable")]:
        with subtest(vm.name + ": read-only discovery and policy TERM fallback"):
            vm.wait_for_unit("multi-user.target")
            try:
                vm.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
            except Exception:
                print(vm.succeed("cat /tmp/rsdm-wayfire.log; journalctl --no-pager _UID=1000"))
                raise
            values = dict(line.split("=", 1) for line in vm.succeed(user + "systemctl --user show-environment").splitlines())
            directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
            record = tomllib.loads(vm.succeed("cat " + directory + "/session.toml"))
            assert dict(record["exported_environment"])["WAYFIRE_SOCKET"] == values["WAYFIRE_SOCKET"]
            compositor = record["compositor_unit"]
            compositor_state = vm.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
            config = vm.succeed("cat /etc/rsdm-wayfire.ini")
            session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " WAYFIRE_SOCKET=" + shlex.quote(values["WAYFIRE_SOCKET"]) + " GSK_RENDERER=cairo "
            status = vm.succeed(session + "rsdm session status")
            assert "manager: full\n" in status, status
            assert "window-close: unavailable (WAYFIRE_SOCKET:" in status, status
            assert reason in status, status

            vm.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window --setenv=GSK_RENDERER=cairo ${application} foreign")
            vm.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${application} owned")
            vm.wait_for_file("/run/user/1000/owned-ready", timeout=30)
            vm.wait_for_file("/run/user/1000/foreign-ready", timeout=30)
            if vm is machine:
                vm.wait_until_succeeds(session + "python3 ${query} | grep 'owned 1'")
                vm.wait_until_succeeds(session + "python3 ${query} | grep 'foreign 1'")
                views = json.loads(vm.succeed(session + "python3 ${query}"))
                foreign = {view["id"] for view in views if view["title"].startswith("foreign ")}
                assert len(foreign) == 2
                # With existing views, StatusDetails also exercises view-info.
                assert reason in vm.succeed(session + "rsdm session status")
            app = tomllib.loads(vm.succeed("cat " + directory + "/apps/*.toml"))
            vm.succeed(user + "systemctl --user stop " + app["unit"])
            vm.wait_for_file("/run/user/1000/owned-term")
            vm.wait_for_file(directory + "/results/" + app["unit"] + ".toml")
            saved = tomllib.loads(vm.succeed("cat " + directory + "/results/" + app["unit"] + ".toml"))
            assert saved["dispatch"]["method"] == "term"
            assert saved["dispatch"]["state"] == "sent"
            assert saved["preparation"]["outcome"] == "clean"
            assert not saved["preparation"]["escalated"]
            vm.fail("test -e /run/user/1000/owned-closes")
            vm.fail("test -e /run/user/1000/foreign-closes")
            vm.fail("test -e /run/user/1000/foreign-term")
            vm.succeed(user + "systemctl --user is-active --quiet rsdm-foreign-window")
            if vm is machine:
                views = json.loads(vm.succeed(session + "python3 ${query}"))
                assert foreign == {view["id"] for view in views if view["title"].startswith("foreign ")}
            assert vm.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == compositor_state
            assert vm.succeed("cat /etc/rsdm-wayfire.ini") == config
            vm.succeed("touch /run/user/1000/foreign-finish")
            vm.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-foreign-window")
            vm.succeed(session + "rsdm session stop")
            vm.wait_for_file("/tmp/rsdm-wayfire-exited")
            vm.wait_until_fails(user + "systemctl --user is-active --quiet " + compositor)
  '';
}
