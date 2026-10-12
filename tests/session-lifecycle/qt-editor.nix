{ pkgs }:

''
    def unit_windows(unit):
        group = machine.succeed(user + "systemctl --user show " + unit + " -p ControlGroup --value").strip()
        assert group.startswith("/") and group != "/", group
        pids = {int(pid) for pid in machine.succeed("find " + shlex.quote("/sys/fs/cgroup" + group) + " -name cgroup.procs -exec cat {} +").split()}
        return [window for window in windows() if window["pid"] in pids]

    with subtest("Kate preserves unsaved text and a foreign editor across repeated logout"):
        machine.succeed("install -o alice -g alice -m600 /dev/null /home/alice/owned.txt")
        machine.succeed("install -o alice -g alice -m600 /dev/null /home/alice/foreign.txt")
        machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-kate --property=ExitType=cgroup --setenv=QT_QPA_PLATFORM=wayland --setenv=WAYLAND_DISPLAY=" + values["WAYLAND_DISPLAY"] + " -- ${pkgs.kdePackages.kate}/bin/kate --startanon /home/alice/foreign.txt")
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- env QT_QPA_PLATFORM=wayland ${pkgs.kdePackages.kate}/bin/kate --startanon /home/alice/owned.txt")
        app = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        for _ in range(30):
            owned = unit_windows(app["unit"])
            foreign_editor = unit_windows("rsdm-foreign-kate.service")
            if owned and foreign_editor:
                break
            machine.sleep(1)
        assert len(owned) == len(foreign_editor) == 1, (owned, foreign_editor)
        assert owned[0]["shell"] == foreign_editor[0]["shell"] == "xdg_shell"
        machine.succeed(session + "swaymsg '[con_id=" + str(owned[0]["id"]) + "] focus'")
        machine.succeed(session + "wtype 'Unsaved Qt editor contents'")
        machine.wait_until_succeeds(session + "swaymsg -t get_tree -r | grep 'owned.txt \\*'")
        foreign_before = machine.succeed(user + "systemctl --user show rsdm-foreign-kate.service -p MainPID -p InvocationID")
        foreign_ids = {window["id"] for window in foreign_editor}
        machine.fail(session + "rsdm session stop")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/apps/" + app["unit"] + ".toml"))
        assert saved["dispatch"]["method"] == "window_close", saved
        assert saved["dispatch"]["state"] == "sent", saved
        assert saved["preparation"]["outcome"] == "cancelled", saved
        assert not saved["preparation"]["escalated"], saved
        own_ids = {window["id"] for window in unit_windows(app["unit"])}
        assert len(own_ids) == 2
        assert any(window["name"] == "Close Document — Kate" for window in unit_windows(app["unit"]))
        machine.succeed("test ! -s /home/alice/owned.txt")
        machine.fail(session + "rsdm session stop")
        assert {window["id"] for window in unit_windows(app["unit"])} == own_ids
        assert {window["id"] for window in unit_windows("rsdm-foreign-kate.service")} == foreign_ids
        assert machine.succeed(user + "systemctl --user show rsdm-foreign-kate.service -p MainPID -p InvocationID") == foreign_before
        assert machine.succeed(user + "systemctl --user show " + compositor + " -p MainPID -p InvocationID") == compositor_state
        # Finish this fixture explicitly after checking that RSDM preserved its dialog.
        machine.succeed(user + "systemctl --user kill --kill-whom=all --signal=KILL " + app["unit"] + " rsdm-foreign-kate.service")
        machine.wait_until_fails("test -e " + directory + "/apps/" + app["unit"] + ".toml")
''
