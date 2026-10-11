{ pkgs }:

let
  python = pkgs.python3.withPackages (packages: [ packages.pygobject3 ]);
  application = pkgs.writeText "wayland-window-app.py" ''
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
in pkgs.writeShellScript "wayland-window-app" ''
  export GI_TYPELIB_PATH=${pkgs.lib.makeSearchPathOutput "out" "lib/girepository-1.0" [ pkgs.gtk4 pkgs.pango pkgs.graphene pkgs.gdk-pixbuf pkgs.gobject-introspection pkgs.glib pkgs.harfbuzz ]}
  export GDK_BACKEND=wayland
  exec ${python}/bin/python3 ${application} "$@"
''
