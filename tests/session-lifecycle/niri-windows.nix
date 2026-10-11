{ pkgs }:

let
  python = pkgs.python3.withPackages (packages: [ packages.pygobject3 ]);
  application = pkgs.writeText "niri-save-dialog.py" ''
    import os
    import signal
    import sys
    from pathlib import Path
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gio, GLib, Gtk

    prefix = sys.argv[1]
    directory = Path(os.environ["XDG_RUNTIME_DIR"])
    app = Gtk.Application(application_id="org.rsdm.WindowTest", flags=Gio.ApplicationFlags.NON_UNIQUE)

    def close(window):
        with (directory / (prefix + "-closes")).open("a") as marker:
            marker.write("close\n")
        if prefix == "clean":
            app.quit()
            return False
        if prefix == "linger":
            window.destroy()
            return True
        dialog = Gtk.Window(application=app, title=prefix + " save", transient_for=window)
        dialog.set_default_size(160, 100)
        dialog.present()
        return True

    def activate(application):
        for number in range(2):
            window = Gtk.ApplicationWindow(application=application, title=prefix + " " + str(number))
            window.set_default_size(200, 120)
            window.connect("close-request", close)
            window.present()

    def finished():
        if (directory / (prefix + "-finish")).exists():
            app.quit()
        return True

    def terminated(signum, frame):
        (directory / (prefix + "-term")).touch()
        app.quit()

    app.connect("activate", activate)
    signal.signal(signal.SIGTERM, terminated)
    GLib.timeout_add(50, finished)
    app.run([sys.argv[0]])
  '';
  command = pkgs.writeShellScript "niri-save-dialog" ''
    export GI_TYPELIB_PATH=${pkgs.lib.makeSearchPathOutput "out" "lib/girepository-1.0" [ pkgs.gtk4 pkgs.pango pkgs.graphene pkgs.gdk-pixbuf pkgs.gobject-introspection pkgs.glib pkgs.harfbuzz ]}
    export GDK_BACKEND=wayland
    exec ${python}/bin/python3 ${application} "$@"
  '';
in {
  inherit command;
  testScript = ''
    import json
    import tomllib

    def windows():
        return json.loads(machine.succeed(session + "niri msg --json windows"))

    with subtest("one Niri close preserves the save dialog and foreign windows across cancellation"):
        machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window ${command} foreign")
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${command} owned")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'owned 1'")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'foreign 1'")
        before = {window["id"] for window in windows() if window["title"].startswith("foreign ")}
        assert len(before) == 2
        machine.fail(session + "rsdm session stop")
        machine.wait_for_file("/run/user/1000/owned-closes")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["title"].startswith("owned ")]) == 3
        machine.fail("test -e /run/user/1000/owned-term")
        machine.fail("test -e /run/user/1000/foreign-closes")
        assert before == {window["id"] for window in windows() if window["title"].startswith("foreign ")}
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/owned-term")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service rsdm-foreign-window")
        directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        assert record["window_close_sent"]
        assert record["dispatch"]["method"] == "window_close"
        assert record["dispatch"]["state"] == "sent"
        assert record["preparation"]["outcome"] == "cancelled"
        machine.succeed("touch /run/user/1000/owned-finish /run/user/1000/foreign-finish")
        machine.wait_until_fails(session + "niri msg --json windows | grep 'owned '")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-foreign-window")
        machine.wait_until_fails("test -e " + directory + "/apps/" + record["unit"] + ".toml")

    with subtest("individual GTK stop closes a window and records clean cgroup exit"):
        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${command} clean")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'clean 1'")
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.succeed(user + "systemctl --user stop " + record["unit"])
        machine.wait_until_succeeds("test -e " + directory + "/results/" + record["unit"] + ".toml")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + record["unit"] + ".toml"))
        assert saved["preparation"]["outcome"] == "clean"
        assert not saved["preparation"]["escalated"]
        assert machine.succeed("cat /run/user/1000/clean-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/clean-term")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service")

    with subtest("a vanished window does not prove application exit or close its sibling"):
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${command} linger")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'linger 1'")
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.fail(session + "rsdm session stop")
        remaining = [window for window in windows() if window["title"].startswith("linger ")]
        assert len(remaining) == 1
        assert machine.succeed("cat /run/user/1000/linger-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/linger-term")
        machine.succeed(user + "systemctl --user is-active --quiet " + record["unit"] + " niri.service")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/apps/" + record["unit"] + ".toml"))
        assert saved["preparation"]["outcome"] == "cancelled"
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/linger-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["title"].startswith("linger ")]) == 1
        machine.succeed("touch /run/user/1000/linger-finish")
        machine.wait_until_fails("test -e " + directory + "/apps/" + record["unit"] + ".toml")
  '';
}
