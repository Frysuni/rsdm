use std::{
    fs, os::unix::fs::PermissionsExt, process::Stdio,
    thread, time::{Duration, Instant},
};

use rsdm_system::process_handle::ProcessHandle;

use super::CliTest;

#[test]
fn inactive_and_failed_user_units_keep_their_reported_state() {
    for state in ["inactive", "failed"] {
        let cli = CliTest::new();
        let path = cli.root.join("systemctl");
        fs::write(&path, format!("#!/bin/sh\nprintf '{state}\\n'\nexit 3\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let result = cli.pipe(&["status"]);
        assert!(result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains(&format!("service: {state}")));
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn status_queries_do_not_wait_for_hung_processes_or_inherited_stdout() {
    for (script, state) in [
        ("printf '%s\\n' \"$$\" > \"$HOME/systemctl-pid\"\nkill -STOP $$", "unavailable"),
        ("/bin/sh -c 'printf \"%s\\n\" \"$$\" > \"$HOME/systemctl-pid\"; kill -STOP $$' &\nprintf 'active\\n'", "active"),
    ] {
        let cli = CliTest::new();
        let path = cli.root.join("systemctl");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let mut process = cli.command(&["status"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(7);
        let pid_file = cli.root.join("systemctl-pid");
        while !pid_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let pid = fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
        let helper = ProcessHandle::open(pid).unwrap().unwrap();
        let mut finished = false;
        while Instant::now() < deadline {
            if process.try_wait().unwrap().is_some() {
                finished = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !finished {
            let _ = process.kill();
        }
        let alive = helper.alive().unwrap();
        // Clean up only this fixture's pinned helper, including the descendant
        // deliberately left holding stdout after its parent exits normally.
        let _ = helper.signal(libc::SIGKILL);
        let output = process.wait_with_output().unwrap();
        assert!(finished, "systemctl prevented the status report from completing");
        assert!(output.status.success());
        let report = String::from_utf8_lossy(&output.stdout);
        assert!(report.contains(&format!("service: {state}")), "{report}");
        assert!(report.contains("active lock: none"), "{report}");
        if state == "unavailable" { assert!(!alive, "timed-out query survived cleanup"); }
    }
}
