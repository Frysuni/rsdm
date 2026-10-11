{ pkgs }:
pkgs.writeShellScript "test-session-controller" ''
  set -eu
  case "$1" in
    system)
      pid=$(systemctl show "$2" --property=MainPID --value)
      ;;
    published)
      session=$(runuser -u "$2" -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus systemctl --user show-environment | sed -n 's/^XDG_SESSION_ID=//p')
      pid=$(loginctl show-session "$session" --property=Leader --value)
      ;;
    *) exit 2 ;;
  esac
  shift 2
  test "$pid" -gt 0
  group=$(sed -n 's/^0:://p' "/proc/$pid/cgroup")
  test -n "$group"
  test "$group" != /
  printf '%s\n' "$$" > "/sys/fs/cgroup$group/cgroup.procs"
  exec runuser -u alice -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "$@"
''
