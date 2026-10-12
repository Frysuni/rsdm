import os
import pathlib
import signal
import time

import dbus


directory = pathlib.Path(os.environ["XDG_RUNTIME_DIR"])
scope = f"app-rsdm-test-migrated-{os.getpid()}.scope"


def stop(_signal, _frame):
    (directory / "scope-app-stopped").write_text("stopped\n")
    raise SystemExit(0)


signal.signal(signal.SIGTERM, stop)
(directory / "scope-app-ready").write_text(str(os.getpid()))
(directory / "scope-app-unit").write_text(scope)
manager = dbus.Interface(
    dbus.SessionBus().get_object("org.freedesktop.systemd1", "/org/freedesktop/systemd1"),
    "org.freedesktop.systemd1.Manager",
)
manager.StartTransientUnit(
    scope,
    "fail",
    dbus.Array([("PIDs", dbus.Array([dbus.UInt32(os.getpid())], signature="u"))], signature="(sv)"),
    dbus.Array([], signature="(sa(sv))"),
)
while True:
    time.sleep(1)
