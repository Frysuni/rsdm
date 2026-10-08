//! Private file fixtures never write kernel settings or signal PID 1.

use std::{
    os::unix::{fs::{PermissionsExt, symlink}, process::ExitStatusExt},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::*;

const ORIGINAL: &str = "7 4 1 7\n";
const QUIET: &str = "1\t4\t1\t7\n";

struct Fixture {
    path: PathBuf,
    printk: PathBuf,
    runtime: PathBuf,
    uid: u32,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("rsdm-console-log-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let printk = path.join("printk");
        fs::write(&printk, ORIGINAL).unwrap();
        let runtime = path.join("runtime");
        // SAFETY: geteuid has no preconditions; fixtures belong to this user.
        let uid = unsafe { libc::geteuid() };
        Self { path, printk, runtime, uid }
    }

    fn quiet(&self) -> io::Result<PrintkGuard> {
        PrintkGuard::quiet_at(&self.printk, &self.runtime, self.uid)
    }

    fn saved(&self) -> PathBuf { self.runtime.join("console-printk.state") }

    fn install_snapshot(&self, text: &str) {
        fs::create_dir_all(&self.runtime).unwrap();
        fs::write(self.saved(), text).unwrap();
        fs::set_permissions(self.saved(), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.path); }
}

#[test]
fn quieting_preserves_all_loglevels_and_restores_on_normal_exit() {
    let fixture = Fixture::new();
    let guard = fixture.quiet().unwrap();
    assert!(!guard.recovered);
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), QUIET);
    assert_eq!(fs::read_to_string(fixture.saved()).unwrap(), ORIGINAL);
    assert_eq!(fs::metadata(fixture.saved()).unwrap().mode() & 0o777, 0o600);
    drop(guard);
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
    assert!(!fixture.saved().exists());
    assert!(fixture.runtime.join("console-printk.lock").exists());
}

#[test]
fn crash_recovery_keeps_the_original_instead_of_the_quiet_value() {
    let fixture = Fixture::new();
    let mut owner = CrashOwner(Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact", "screens::login::interactive::console_log::tests::snapshot_crash_fixture",
            "--ignored", "--nocapture", "--test-threads=1",
        ])
        .env("RSDM_TEST_CONSOLE_LOG", &fixture.path)
        .stdout(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.path.join("ready").exists() {
        assert!(owner.0.try_wait().unwrap().is_none(), "snapshot fixture exited early");
        assert!(Instant::now() < deadline, "snapshot fixture did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), QUIET);
    owner.0.kill().unwrap();
    assert_eq!(owner.0.wait().unwrap().signal(), Some(libc::SIGKILL));

    let recovered = fixture.quiet().unwrap();
    assert!(recovered.recovered);
    assert_eq!(fs::read_to_string(fixture.saved()).unwrap(), ORIGINAL);
    drop(recovered);
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
    assert!(!fixture.saved().exists());
}

struct CrashOwner(Child);

impl Drop for CrashOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "isolated subprocess fixture for console logging crash recovery"]
fn snapshot_crash_fixture() {
    let path = PathBuf::from(std::env::var_os("RSDM_TEST_CONSOLE_LOG").unwrap());
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let _guard = PrintkGuard::quiet_at(&path.join("printk"), &path.join("runtime"), uid).unwrap();
    fs::write(path.join("ready"), "ready").unwrap();
    loop { thread::sleep(Duration::from_secs(60)); }
}

#[test]
fn competing_greeters_never_replace_a_live_baseline() {
    let fixture = Fixture::new();
    let guard = fixture.quiet().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact", "screens::login::interactive::console_log::tests::snapshot_contender_fixture",
            "--ignored", "--test-threads=1",
        ])
        .env("RSDM_TEST_CONSOLE_LOG", &fixture.path).status().unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(fixture.saved()).unwrap(), ORIGINAL);
    drop(guard);
    let next = fixture.quiet().unwrap();
    assert!(!next.recovered);
    drop(next);
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
}

#[test]
#[ignore = "isolated subprocess fixture for competing console logging owners"]
fn snapshot_contender_fixture() {
    let path = PathBuf::from(std::env::var_os("RSDM_TEST_CONSOLE_LOG").unwrap());
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let result = PrintkGuard::quiet_at(&path.join("printk"), &path.join("runtime"), uid);
    assert!(matches!(result, Err(error) if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EAGAIN))));
}

#[test]
fn forked_session_owner_cannot_keep_the_console_lease_alive() {
    let fixture = Fixture::new();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact", "screens::login::interactive::console_log::tests::snapshot_fork_fixture",
            "--ignored", "--test-threads=1",
        ])
        .env("RSDM_TEST_CONSOLE_LOG", &fixture.path).output().unwrap();
    assert!(output.status.success(), "{}{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "isolated subprocess fixture for inherited console logging descriptors"]
fn snapshot_fork_fixture() {
    let path = PathBuf::from(std::env::var_os("RSDM_TEST_CONSOLE_LOG").unwrap());
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let guard = PrintkGuard::quiet_at(&path.join("printk"), &path.join("runtime"), uid).unwrap();
    // SAFETY: the child only pauses in libc until killed; no Rust or destructor
    // runs in the child, matching the detached PAM owner's inherited handles.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        loop { unsafe { libc::pause(); } }
    }
    let _child = ForkOwner(pid);
    drop(guard);
    let next = PrintkGuard::quiet_at(&path.join("printk"), &path.join("runtime"), uid).unwrap();
    assert!(!next.recovered);
    drop(next);
    assert_eq!(fs::read_to_string(path.join("printk")).unwrap(), ORIGINAL);
}

struct ForkOwner(libc::pid_t);

impl Drop for ForkOwner {
    fn drop(&mut self) {
        // SAFETY: terminate and reap only our still-unreaped fixture child.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, std::ptr::null_mut(), 0);
        }
    }
}

#[test]
fn malformed_and_oversized_snapshots_do_not_change_kernel_settings() {
    for text in ["7 4\n".into(), "7 4 1 invalid\n".into(), "7 4 1 7\n".repeat(20)] {
        let fixture = Fixture::new();
        fixture.install_snapshot(&text);
        assert!(fixture.quiet().is_err());
        assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
        assert_eq!(fs::read_to_string(fixture.saved()).unwrap(), text);
    }
}

#[test]
fn untrusted_snapshot_files_and_directories_are_rejected() {
    let fixture = Fixture::new();
    fixture.install_snapshot(ORIGINAL);
    fs::set_permissions(fixture.saved(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(fixture.quiet().is_err());
    fs::set_permissions(fixture.saved(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(fixture.saved(), fixture.path.join("alias")).unwrap();
    assert!(fixture.quiet().is_err());
    fs::remove_file(fixture.saved()).unwrap();
    symlink(fixture.path.join("alias"), fixture.saved()).unwrap();
    assert!(fixture.quiet().is_err());
    fs::remove_file(fixture.saved()).unwrap();
    symlink(&fixture.printk, fixture.runtime.join("console-printk.lock")).unwrap_err();
    fs::remove_file(fixture.runtime.join("console-printk.lock")).unwrap();
    symlink(&fixture.printk, fixture.runtime.join("console-printk.lock")).unwrap();
    assert!(fixture.quiet().is_err());
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);

    fs::remove_dir_all(&fixture.runtime).unwrap();
    symlink(&fixture.path, &fixture.runtime).unwrap();
    assert!(fixture.quiet().is_err());
    fs::remove_file(&fixture.runtime).unwrap();
    fs::create_dir(&fixture.runtime).unwrap();
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(fixture.quiet().is_err());
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
}

#[test]
fn failed_restore_retains_a_snapshot_for_the_next_greeter() {
    let fixture = Fixture::new();
    let guard = fixture.quiet().unwrap();
    fs::remove_file(&fixture.printk).unwrap();
    fs::create_dir(&fixture.printk).unwrap();
    drop(guard);
    assert_eq!(fs::read_to_string(fixture.saved()).unwrap(), ORIGINAL);
    fs::remove_dir(&fixture.printk).unwrap();
    fs::write(&fixture.printk, QUIET).unwrap();
    let recovered = fixture.quiet().unwrap();
    assert!(recovered.recovered);
    drop(recovered);
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), ORIGINAL);
    assert!(!fixture.saved().exists());
}

#[test]
fn invalid_live_settings_cannot_become_a_recovery_baseline() {
    let fixture = Fixture::new();
    fs::write(&fixture.printk, "invalid").unwrap();
    assert!(fixture.quiet().is_err());
    assert!(!fixture.saved().exists());
    assert_eq!(fs::read_to_string(&fixture.printk).unwrap(), "invalid");
}
