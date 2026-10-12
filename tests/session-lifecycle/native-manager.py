"""Native manager protocol fixture; no compositor or system power requests."""

import os
from pathlib import Path
import subprocess
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


RUNTIME = Path(os.environ["XDG_RUNTIME_DIR"])
GNOME = "org.gnome.SessionManager"
PLASMA = "org.kde.LogoutPrompt"


class Manager(dbus.service.Object):
    def __init__(self, bus, desktop, loop, role):
        self.loop = loop
        self.names = []
        if role != "prompt-only":
            self.names.append(dbus.service.BusName("org.rsdm.TestNative", bus))
        if desktop == "GNOME":
            self.names.append(dbus.service.BusName(GNOME, bus))
            path = "/org/gnome/SessionManager"
        else:
            if role != "prompt-only":
                self.names.append(dbus.service.BusName("org.kde.ksmserver", bus))
            if role != "on-demand":
                self.names.append(dbus.service.BusName(PLASMA, bus))
            path = "/LogoutPrompt"
        super().__init__(bus, path)

    def request(self, action):
        required = RUNTIME / "native-require-saved"
        if required.exists() and (RUNTIME / required.read_text().strip()).read_text() != "saved\n":
            raise dbus.exceptions.DBusException(
                "registered application was not prepared", name="org.freedesktop.DBus.Error.Failed"
            )
        if (RUNTIME / "native-deny").exists():
            raise dbus.exceptions.DBusException(
                "native request denied", name="org.freedesktop.DBus.Error.AccessDenied"
            )
        with (RUNTIME / "native-requests").open("a") as report:
            report.write(action + "\n")

    @dbus.service.method(GNOME, in_signature="", out_signature="b")
    def IsSessionRunning(self):
        return True

    @dbus.service.method(GNOME, in_signature="u", out_signature="")
    def Logout(self, mode):
        assert mode == 0, "logout must preserve native confirmation and inhibitors"
        self.request("logout")

    @dbus.service.method(GNOME, in_signature="", out_signature="")
    def Reboot(self):
        self.request("reboot")

    @dbus.service.method(GNOME, in_signature="", out_signature="")
    def Shutdown(self):
        self.request("poweroff")

    @dbus.service.method(PLASMA, in_signature="", out_signature="")
    def promptLogout(self):
        self.request("logout")

    @dbus.service.method(PLASMA, in_signature="", out_signature="")
    def promptReboot(self):
        self.request("reboot")

    @dbus.service.method(PLASMA, in_signature="", out_signature="")
    def promptShutDown(self):
        self.request("poweroff")

    @dbus.service.method("org.rsdm.TestNative", in_signature="", out_signature="")
    def Activate(self):
        subprocess.run(["systemctl", "--user", "start", "test-native.target"], check=True)

    @dbus.service.method("org.rsdm.TestNative", in_signature="", out_signature="")
    def ExitManager(self):
        self.loop.quit()

    @dbus.service.method("org.rsdm.TestNative", in_signature="", out_signature="s")
    def VerifyHandshake(self):
        return subprocess.check_output([
            "busctl", "--user", "call", "org.rsdm.Session1", "/org/rsdm/Session1",
            "org.rsdm.Session1", "Handshake",
        ], text=True).strip()

    @dbus.service.method("org.rsdm.TestNative", in_signature="", out_signature="")
    def Finish(self):
        subprocess.run(["systemctl", "--user", "stop", "test-native.target", "graphical-session.target"], check=True)
        (RUNTIME / "display-alive").unlink()
        self.loop.quit()


DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
loop = GLib.MainLoop()
role = sys.argv[2] if len(sys.argv) > 2 else "desktop"
manager = Manager(bus, sys.argv[1], loop, role)
if role == "prompt-only":
    (RUNTIME / "native-prompt-started").touch()
else:
    (RUNTIME / "display-alive").touch()
    if role != "defer":
        manager.Activate()
    (RUNTIME / "native-manager-ready").touch()
loop.run()
