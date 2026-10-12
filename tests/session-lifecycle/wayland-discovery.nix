{ pkgs, rsdm }:

let
  controller = import ./controller.nix { inherit pkgs; };
  application = import ./window-app.nix { inherit pkgs; };
  node = command: renderer: { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    hardware.graphics.enable = true;
    security.polkit.enable = true;
    users.users.alice = { isNormalUser = true; uid = 1000; group = "alice"; };
    users.groups.alice = {};
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ rsdm pkgs.labwc pkgs.river pkgs.wlrctl ];
    environment.etc."rsdm.toml".text = "";
    environment.etc."rsdm-labwc/autostart" = {
      mode = "0755";
      text = "#!${pkgs.runtimeShell}\n${rsdm}/bin/rsdm session finalize\n";
    };
    environment.variables = {
      WLR_BACKENDS = "headless";
      WLR_RENDERER = renderer;
      WLR_RENDERER_ALLOW_SOFTWARE = "1";
      LIBGL_ALWAYS_SOFTWARE = "1";
    };
    programs.bash.loginShellInit = ''
      if [ "$(tty)" = /dev/tty1 ] && [ -z "''${RSDM_SESSION_GENERATION:-}" ]; then
        ${rsdm}/bin/rsdm session start -- ${command} > /tmp/rsdm-wayland.log 2>&1
        touch /tmp/rsdm-wayland-exited
      fi
    '';
  };
in pkgs.testers.runNixOSTest {
  name = "rsdm-wayland-discovery";
  globalTimeout = 180;
  nodes.labwc = node "${pkgs.labwc}/bin/labwc -C /etc/rsdm-labwc" "gles2";
  nodes.river = node "${pkgs.river}/bin/river -c '${rsdm}/bin/rsdm session finalize'" "pixman";
  testScript = ''
    import shlex
    import tomllib

    start_all()
    user = "runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
    control = "${controller} published alice "
    for vm in [labwc, river]:
        with subtest(vm.name + ": authenticated registry discovery"):
            vm.wait_for_unit("multi-user.target")
            try:
                vm.wait_until_succeeds(control + "rsdm session status | grep '^managed: running '", timeout=60)
            except Exception:
                print(vm.succeed("cat /tmp/rsdm-wayland.log; journalctl --no-pager _UID=1000"))
                raise
            values = dict(line.split("=", 1) for line in vm.succeed(user + "systemctl --user show-environment").splitlines())
            directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
            record = tomllib.loads(vm.succeed("cat " + directory + "/session.toml"))
            assert dict(record["exported_environment"])["WAYLAND_DISPLAY"] == values["WAYLAND_DISPLAY"]
            compositor = record["compositor_unit"]
            before = vm.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
            session = control + "RSDM_SESSION_GENERATION=" + values["RSDM_SESSION_GENERATION"] + " WAYLAND_DISPLAY=" + shlex.quote(values["WAYLAND_DISPLAY"]) + " GSK_RENDERER=cairo "
            status = vm.succeed(session + "rsdm session status")
            assert "manager: full\n" in status, status
            assert "window-close: unavailable" in status, status
            assert "WAYLAND_DISPLAY: advertised" in status, status
            assert "ext_foreign_toplevel_list_v1 v1" in status, status
            assert "no verified window PID/cgroup or close handle" in status, status
            if vm is labwc:
                assert "zwlr_foreign_toplevel_manager_v1 v3" in status, status
                config = vm.succeed("cat /etc/rsdm-labwc/autostart")
                vm.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window --setenv=GSK_RENDERER=cairo ${application} foreign")
                vm.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${application} owned")
                vm.wait_for_file("/run/user/1000/owned-ready", timeout=30)
                vm.wait_for_file("/run/user/1000/foreign-ready", timeout=30)
                vm.wait_until_succeeds(session + "wlrctl toplevel list | grep 'owned 1'")
                vm.wait_until_succeeds(session + "wlrctl toplevel list | grep 'foreign 1'")
                assert "window-close: unavailable" in vm.succeed(session + "rsdm session status")
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
                assert "foreign 1" in vm.succeed(session + "wlrctl toplevel list")
                assert config == vm.succeed("cat /etc/rsdm-labwc/autostart")
            assert before == vm.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID")
            assert "window-close: unavailable" in vm.succeed(session + "rsdm session status")
            vm.succeed(session + "rsdm session stop | grep 'stopped'")
            vm.wait_for_file("/tmp/rsdm-wayland-exited")
            vm.fail(user + "systemctl --user is-active --quiet " + compositor)
  '';
}
