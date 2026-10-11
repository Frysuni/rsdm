"""Independent native WM lifetime for launcher observation tests."""

import os
from pathlib import Path
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


RUNTIME = Path(os.environ["XDG_RUNTIME_DIR"])


class Compositor(dbus.service.Object):
    def __init__(self, bus, desktop, loop):
        self.loop = loop
        name = "org.gnome.Shell" if desktop == "GNOME" else "org.kde.KWin"
        self.native = dbus.service.BusName(name, bus)
        self.control = dbus.service.BusName("org.rsdm.TestCompositor", bus)
        super().__init__(bus, "/org/rsdm/TestCompositor")

    @dbus.service.method("org.rsdm.TestCompositor", in_signature="", out_signature="")
    def ReleaseName(self):
        self.native = None

    @dbus.service.method("org.rsdm.TestCompositor", in_signature="", out_signature="")
    def Finish(self):
        (RUNTIME / "display-alive").unlink()
        self.loop.quit()


DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
loop = GLib.MainLoop()
compositor = Compositor(bus, sys.argv[1], loop)
(RUNTIME / "display-alive").touch()
(RUNTIME / "native-compositor-ready").touch()
loop.run()
