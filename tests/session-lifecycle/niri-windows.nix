{ pkgs }:

let
  command = import ./window-app.nix { inherit pkgs; };
in {
  inherit command;
  testScript = ''
    import json
    import tomllib

    def windows():
        return json.loads(machine.succeed(session + "niri msg --json windows"))

    with subtest("one Niri close preserves the save dialog and foreign windows across cancellation"):
        machine.succeed(user + "systemd-run --user --collect --unit=rsdm-foreign-window ${command} foreign")
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${command} owned")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'owned 1'")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'foreign 1'")
        before = {window["id"] for window in windows() if window["title"].startswith("foreign ")}
        assert len(before) == 2
        status = machine.succeed(session + "rsdm session status")
        assert "manager: full\n" in status, status
        assert "window-close: available (NIRI_SOCKET:" in status, status
        machine.fail("test -e /run/user/1000/owned-closes")
        machine.fail("test -e /run/user/1000/foreign-closes")
        machine.fail(session + "rsdm session stop")
        machine.wait_for_file("/run/user/1000/owned-closes")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["title"].startswith("owned ")]) == 3
        machine.fail("test -e /run/user/1000/owned-term")
        machine.fail("test -e /run/user/1000/foreign-closes")
        assert before == {window["id"] for window in windows() if window["title"].startswith("foreign ")}
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/owned-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/owned-term")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service rsdm-foreign-window")
        directory = "/run/user/1000/rsdm/sessions/" + values["RSDM_SESSION_GENERATION"]
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        assert record["original_process"]["pidfd_id"] > 0
        assert record["original_process"]["pidfd_id"] == int(machine.succeed("cat /run/user/1000/owned-pidfd-id"))
        assert record["original_process"]["pid"] == int(machine.succeed(user + "systemctl --user show " + record["unit"] + " -p MainPID --value"))
        assert record["window_close_sent"]
        assert record["dispatch"]["method"] == "window_close"
        assert record["dispatch"]["state"] == "sent"
        assert record["preparation"]["outcome"] == "cancelled"
        status = machine.succeed(session + "rsdm session status")
        assert "cancelled, method window-close, dispatch sent, escalated false" in status, status
        machine.succeed("touch /run/user/1000/owned-finish /run/user/1000/foreign-finish")
        machine.wait_until_fails(session + "niri msg --json windows | grep 'owned '")
        machine.wait_until_fails(user + "systemctl --user is-active --quiet rsdm-foreign-window")
        machine.wait_until_fails("test -e " + directory + "/apps/" + record["unit"] + ".toml")

    with subtest("individual GTK stop closes a window and records clean cgroup exit"):
        machine.succeed(session + "rsdm app --shutdown-timeout 5 --on-timeout cancel -- ${command} clean")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'clean 1'")
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.succeed(user + "systemctl --user stop " + record["unit"])
        machine.wait_until_succeeds("test -e " + directory + "/results/" + record["unit"] + ".toml")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/results/" + record["unit"] + ".toml"))
        assert saved["preparation"]["outcome"] == "clean"
        assert not saved["preparation"]["escalated"]
        status = machine.succeed(session + "rsdm session status")
        assert "preparation: " + record["unit"] + ": clean, method window-close, dispatch sent, escalated false" in status, status
        assert machine.succeed("cat /run/user/1000/clean-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/clean-term")
        machine.succeed(user + "systemctl --user is-active --quiet niri.service")

    with subtest("a vanished window does not prove application exit or close its sibling"):
        machine.succeed(session + "rsdm app --shutdown-timeout 1 --on-timeout cancel -- ${command} linger")
        machine.wait_until_succeeds(session + "niri msg --json windows | grep 'linger 1'")
        record = tomllib.loads(machine.succeed("cat " + directory + "/apps/*.toml"))
        machine.fail(session + "rsdm session stop")
        remaining = [window for window in windows() if window["title"].startswith("linger ")]
        assert len(remaining) == 1
        assert machine.succeed("cat /run/user/1000/linger-closes").splitlines() == ["close"]
        machine.fail("test -e /run/user/1000/linger-term")
        machine.succeed(user + "systemctl --user is-active --quiet " + record["unit"] + " niri.service")
        saved = tomllib.loads(machine.succeed("cat " + directory + "/apps/" + record["unit"] + ".toml"))
        assert saved["preparation"]["outcome"] == "cancelled"
        machine.fail(session + "rsdm session stop")
        assert machine.succeed("cat /run/user/1000/linger-closes").splitlines() == ["close"]
        assert len([window for window in windows() if window["title"].startswith("linger ")]) == 1
        machine.succeed("touch /run/user/1000/linger-finish")
        machine.wait_until_fails("test -e " + directory + "/apps/" + record["unit"] + ".toml")
  '';
}
