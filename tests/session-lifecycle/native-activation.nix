{ native }:
''
    with subtest("Plasma's on-demand prompt is activated behind its live session manager"):
        machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found")
        machine.succeed("rm -f " + runtime + "/app-* " + runtime + "/native-*")
        machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_CURRENT_DESKTOP=KDE --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- rsdm session start -- ${native} KDE on-demand")
        machine.wait_until_succeeds(control + "rsdm session status | grep '^plasma: running '", timeout=30)
        for attempt in range(3):
            status = machine.succeed(control + "rsdm session status")
            assert "capability: native-logout: available" in status, status
            assert "manager: full\n" in status, status
        machine.succeed(user + "busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus NameHasOwner s org.kde.LogoutPrompt | grep 'b false'")
        machine.fail("test -e " + runtime + "/native-prompt-started")
        for args in ["session stop", "power reboot", "power poweroff"]:
            output = machine.succeed(control + "rsdm " + args)
            assert "delegated" in output, output
            machine.succeed("test -e " + runtime + "/display-alive")
            machine.succeed(control + "rsdm session status | grep '^plasma: running '")
        machine.wait_for_file(runtime + "/native-prompt-started")
        assert machine.succeed("cat " + runtime + "/native-requests") == "logout\nreboot\npoweroff\n"
        machine.succeed(user + "busctl --user call org.kde.LogoutPrompt /LogoutPrompt org.rsdm.TestNative ExitManager")
        machine.succeed(user + "busctl --user call org.rsdm.TestNative /LogoutPrompt org.rsdm.TestNative Finish")
        machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
        machine.fail("test -e " + runtime + "/display-alive")
''
