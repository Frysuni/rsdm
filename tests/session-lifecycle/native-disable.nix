{ python, native, application }:
''
    import tomllib
    ${import ./control-authorization.nix { inherit application; }}

    def start_native_compositor(desktop):
        machine.succeed(user + "systemd-run --user --unit=rsdm-test-native-wm --collect -- ${python}/bin/python3 ${./native-compositor.py} " + desktop)
        machine.wait_for_file(runtime + "/native-compositor-ready")
        return machine.succeed(user + "systemctl --user show rsdm-test-native-wm -p MainPID -p InvocationID")

    for desktop, provider, path in [
        ("GNOME", "gnome", "/org/gnome/SessionManager"),
        ("KDE", "plasma", "/LogoutPrompt"),
    ]:
        for phase in ["starting", "running"]:
            with subtest(provider + " disable in " + phase + " retains PAM until WM exits"):
                machine.wait_until_succeeds("test \"$(systemctl show --property=LoadState --value rsdm-test-session.service)\" = not-found")
                machine.succeed("rm -f " + runtime + "/native-* " + runtime + "/app-* " + runtime + "/display-alive")
                wm = start_native_compositor(desktop) if phase == "running" else None
                mode = " defer" if phase == "starting" else ""
                machine.succeed("systemd-run --unit=rsdm-test-session --collect --uid=alice --property=PAMName=login --property=TTYPath=/dev/tty1 --setenv=XDG_SESSION_TYPE=wayland --setenv=XDG_CURRENT_DESKTOP=" + desktop + " --setenv=XDG_RUNTIME_DIR=" + runtime + " --setenv=DBUS_SESSION_BUS_ADDRESS=unix:path=" + runtime + "/bus -- rsdm session start -- ${native} " + desktop + mode)
                machine.wait_for_file(runtime + "/native-manager-ready")
                machine.wait_until_succeeds(control + "rsdm session status | grep '^" + provider + ": " + phase + " '")
                generation = machine.succeed(user + "systemctl --user show-environment | sed -n 's/^RSDM_SESSION_GENERATION=//p'").strip()
                directory = runtime + "/rsdm/sessions/" + generation
                record = tomllib.loads(machine.succeed("cat " + directory + "/session.toml"))
                handshake = machine.succeed(user + "busctl --user call org.rsdm.TestNative " + path + " org.rsdm.TestNative VerifyHandshake")
                assert generation in handshake, handshake
                impostor = user + "RSDM_SESSION_GENERATION=" + generation + " XDG_SESSION_ID=" + record["identity"]["login_session_id"] + " "
                before = machine.succeed("ls " + directory + "/apps")
                for call in denied_control_calls(generation):
                    assert_control_denied(impostor + shlex.join(["busctl", "--user", "call", "org.rsdm.Session1", "/org/rsdm/Session1", "org.rsdm.Session1"] + call))
                assert machine.succeed("ls " + directory + "/apps") == before
                launcher = machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID")
                if phase == "running":
                    reject_other_login(generation, record["identity"]["login_session_id"])
                    assert machine.succeed("ls " + directory + "/apps") == before
                    machine.succeed(control + "RSDM_SESSION_GENERATION=" + generation + " rsdm app -- ${application} handshake")
                    machine.wait_for_file(runtime + "/app-handshake-ready")
                    assert generation in machine.succeed("cat " + runtime + "/app-handshake-handshake")
                    launcher_pid = machine.succeed("systemctl show rsdm-test-session -p MainPID --value").strip()
                    wm_pid = machine.succeed(user + "systemctl --user show rsdm-test-native-wm -p MainPID --value").strip()
                    machine.wait_until_succeeds("grep -Eq '^Pid:[[:space:]]+" + wm_pid + "$' /proc/" + launcher_pid + "/fdinfo/*")
                    machine.succeed(user + "busctl --user call org.rsdm.TestCompositor /org/rsdm/TestCompositor org.rsdm.TestCompositor ReleaseName")
                    native_name = "org.gnome.Shell" if desktop == "GNOME" else "org.kde.KWin"
                    machine.wait_until_fails(user + "busctl --user list | grep -F " + native_name)
                machine.succeed("printf '[session_manager]\\nenabled = false\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
                machine.wait_until_succeeds("grep -q '^disabled = true$' " + directory + "/session.toml")
                machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.Session1")
                machine.wait_until_fails("systemd-inhibit --list --no-pager | grep 'Saving registered graphical applications'")
                if phase == "starting":
                    wm = start_native_compositor(desktop)
                    launcher_pid = machine.succeed("systemctl show rsdm-test-session -p MainPID --value").strip()
                    wm_pid = machine.succeed(user + "systemctl --user show rsdm-test-native-wm -p MainPID --value").strip()
                    machine.wait_until_succeeds("grep -Eq '^Pid:[[:space:]]+" + wm_pid + "$' /proc/" + launcher_pid + "/fdinfo/*")
                machine.succeed(user + "busctl --user call org.rsdm.TestNative " + path + " org.rsdm.TestNative ExitManager")
                machine.wait_until_fails(user + "busctl --user list | grep org.rsdm.TestNative")
                machine.sleep(2)
                machine.succeed("systemctl is-active --quiet rsdm-test-session")
                machine.succeed("test -e " + runtime + "/display-alive")
                assert machine.succeed("systemctl show rsdm-test-session -p MainPID -p InvocationID") == launcher
                assert machine.succeed(user + "systemctl --user show rsdm-test-native-wm -p MainPID -p InvocationID") == wm
                machine.succeed("loginctl show-session " + record["identity"]["login_session_id"])
                machine.succeed("printf '[session_manager]\\nenabled = true\\n' > /etc/rsdm-toggle.toml; mv /etc/rsdm-toggle.toml /etc/rsdm.toml")
                machine.succeed(user + "RSDM_SESSION_GENERATION=" + generation + " rsdm session cleanup --generation " + generation)
                machine.succeed("systemctl is-active --quiet rsdm-test-session")
                machine.succeed(user + "busctl --user call org.rsdm.TestCompositor /org/rsdm/TestCompositor org.rsdm.TestCompositor Finish")
                machine.wait_until_fails("systemctl is-active --quiet rsdm-test-session")
                machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-test-native-wm")
                machine.fail("test -e " + runtime + "/display-alive")
                machine.succeed(user + "systemctl --user stop test-native.target graphical-session.target")
''
