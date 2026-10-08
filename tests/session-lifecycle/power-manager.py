"""Private logind protocol fixture, backed by a separate test bus."""

import os
from pathlib import Path
import socket
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


RUNTIME = Path(os.environ["XDG_RUNTIME_DIR"])
MANAGER = "org.freedesktop.login1.Manager"
SESSION = "org.freedesktop.login1.Session"


class Manager(dbus.service.Object):
    def __init__(self, bus):
        self.bus = bus
        self.name = dbus.service.BusName("org.freedesktop.login1", bus)
        self.preparing = False
        self.guards = []
        self.released = set()
        self.shutdown_guards = set()
        self.shutdown_started = None
        self.last_release_usec = 0
        super().__init__(bus, "/org/freedesktop/login1")

    @dbus.service.method(MANAGER, in_signature="u", out_signature="o")
    def GetSessionByPID(self, pid):
        return "/org/freedesktop/login1/session/test"

    @dbus.service.method(MANAGER, in_signature="s", out_signature="o")
    def GetSession(self, session):
        return "/org/freedesktop/login1/session/test"

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, name):
        assert interface == MANAGER
        if name == "InhibitDelayMaxUSec":
            return dbus.UInt64(5_000_000)
        if name == "PreparingForShutdown":
            return dbus.Boolean(self.preparing)
        raise dbus.exceptions.DBusException("unknown property")

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        return {name: self.Get(interface, name) for name in ["InhibitDelayMaxUSec", "PreparingForShutdown"]}

    @dbus.service.method(MANAGER, in_signature="ssss", out_signature="h")
    def Inhibit(self, what, who, reason, mode):
        assert (what, who, mode) == ("shutdown", "RSDM", "delay")
        guard, peer = socket.socketpair()
        self.guards.append(peer)
        GLib.io_add_watch(peer.fileno(), GLib.IO_IN | GLib.IO_HUP, self.guard_released, peer)
        result = dbus.types.UnixFd(guard.fileno())
        guard.close()
        return result

    def guard_released(self, source, condition, peer):
        if peer.recv(1, socket.MSG_PEEK) != b"":
            return True
        self.released.add(peer.fileno())
        if peer.fileno() in self.shutdown_guards and self.shutdown_started is not None:
            self.last_release_usec = int((time.monotonic() - self.shutdown_started) * 1_000_000)
        return False

    @dbus.service.method(MANAGER, in_signature="", out_signature="s")
    def CanPowerOff(self):
        return "no" if (RUNTIME / "power-deny-check").exists() else "yes"

    @dbus.service.method(MANAGER, in_signature="b", out_signature="", sender_keyword="sender")
    def PowerOff(self, interactive, sender):
        assert interactive
        uid = self.bus.get_unix_user(sender)
        with (RUNTIME / "power-requests").open("a") as report:
            report.write(str(uid) + "\n")
        if (RUNTIME / "power-deny-request").exists():
            raise dbus.exceptions.DBusException(
                "power request denied", name="org.freedesktop.DBus.Error.AccessDenied"
            )
        self.PrepareForShutdown(True)

    @dbus.service.signal(MANAGER, signature="b")
    def PrepareForShutdown(self, preparing):
        self.preparing = preparing
        if preparing:
            self.shutdown_started = time.monotonic()
            self.shutdown_guards = {peer.fileno() for peer in self.guards} - self.released
            self.last_release_usec = 0

    @dbus.service.method("org.rsdm.TestPower", in_signature="b", out_signature="")
    def Prepare(self, preparing):
        self.PrepareForShutdown(preparing)

    @dbus.service.method("org.rsdm.TestPower", in_signature="", out_signature="u")
    def ReleasedGuards(self):
        released = 0
        for peer in self.guards:
            peer.setblocking(False)
            try:
                if peer.recv(1, socket.MSG_PEEK) == b"":
                    released += 1
            except BlockingIOError:
                pass
        return dbus.UInt32(released)

    @dbus.service.method("org.rsdm.TestPower", in_signature="", out_signature="t")
    def LastShutdownReleaseUsec(self):
        return dbus.UInt64(self.last_release_usec)


class Session(dbus.service.Object):
    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, name):
        assert interface == SESSION
        if name == "User":
            return dbus.Struct((dbus.UInt32(os.geteuid()), dbus.ObjectPath("/org/freedesktop/login1/user/test")))
        if name == "Id":
            return dbus.String("test")
        raise dbus.exceptions.DBusException("unknown property")

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        return {name: self.Get(interface, name) for name in ["User", "Id"]}


DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
manager = Manager(bus)
session = Session(bus, "/org/freedesktop/login1/session/test")
(RUNTIME / "power-manager-ready").touch()
GLib.MainLoop().run()
