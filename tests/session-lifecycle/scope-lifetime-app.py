import os
import pathlib
import signal
import subprocess
import sys
import time

import dbus


directory = pathlib.Path(os.environ["XDG_RUNTIME_DIR"])
scope = f"app-rsdm-test-migrated-{os.getpid()}.scope"
mode = sys.argv[1] if len(sys.argv) > 1 else "single"


def stop(_signal, _frame):
    marker = "scope-child-stopped" if mode == "child" else "scope-app-stopped"
    (directory / marker).write_text("stopped\n")
    raise SystemExit(0)


signal.signal(signal.SIGTERM, stop)
if mode == "child":
    (directory / "scope-child-ready").write_text(str(os.getpid()))
    while True:
        time.sleep(1)

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
if mode == "tree":
    while not any(line.endswith("/" + scope) for line in pathlib.Path("/proc/self/cgroup").read_text().splitlines()):
        time.sleep(0.01)
    subprocess.Popen([sys.executable, __file__, "child"])
    while not (directory / "scope-main-exit").exists():
        time.sleep(0.01)
    raise SystemExit(0)

while True:
    time.sleep(1)
