//! Verify real terminal output without executing desktop or power actions.

use std::{
    fs, io::{self, Read}, os::fd::{FromRawFd, OwnedFd}, path::PathBuf,
    process::{Command, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct CliTest { root: PathBuf }

impl CliTest {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rsdm-output-test-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("config.toml"), "[dm]\nenable = false\n[dm.tty]\npath = '/dev/tty2'\n").unwrap();
        Self { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rsdm"));
        command.env_clear().env("PATH", &self.root).env("HOME", &self.root)
            .env("XDG_RUNTIME_DIR", &self.root).env("TERM", "xterm-256color")
            .arg("--config").arg(self.root.join("config.toml")).args(args);
        command
    }

    fn pipe(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

impl Drop for CliTest {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}

fn terminal(mut command: Command, columns: u16) -> (ExitStatus, String) {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize { ws_row: 24, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: fd outputs and size are live storage; null optional name/termios are supported.
    assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &size) }, 0);
    // SAFETY: openpty returned two fresh descriptors, each transferred to exactly one owner.
    let mut reader = fs::File::from(unsafe { OwnedFd::from_raw_fd(master) });
    // SAFETY: slave is the second owned descriptor from openpty.
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    command.stdout(Stdio::from(slave.try_clone().unwrap())).stderr(Stdio::from(slave));
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => panic!("reading PTY: {error}"),
        }
    }
    (child.wait().unwrap(), String::from_utf8(bytes).unwrap())
}

fn assert_only_styles(text: &str) {
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            assert_eq!(chars.next(), Some('['));
            loop {
                match chars.next().expect("complete escape") {
                    '0'..='9' | ';' => {}
                    'm' => break,
                    other => panic!("unexpected terminal control {other:?}"),
                }
            }
        }
    }
}

#[test]
fn terminal_reports_are_colored_and_do_not_control_the_screen() {
    let cli = CliTest::new();
    let commands: &[&[&str]] = &[
        &["--help"], &["dm", "--help"], &["lock", "--help"], &["idle", "--help"],
        &["unlock", "--help"], &["logs", "--help"], &["status", "--help"], &["session", "--help"],
        &["session", "start", "--help"], &["session", "finalize", "--help"], &["session", "stop", "--help"],
        &["session", "cancel", "--help"], &["session", "status", "--help"], &["app", "--help"],
        &["power", "--help"], &["validate-config", "--help"], &["help", "session", "status"],
        &["status"], &["validate-config"], &["--version"],
    ];
    for args in commands {
        let (status, text) = terminal(cli.command(args), 80);
        assert!(status.success(), "{text}");
        assert!(text.contains("RSDM"), "{text}");
        assert!(text.contains("\x1b["), "{text}");
        assert!(text.contains('╭') && text.contains('╰'), "{text}");
        if *args == ["status"] {
            assert!(text.contains("\x1b[48;2;148;163;184m"), "{text}");
            assert!(text.contains("\x1b[49m"), "background must reset after the badge");
        }
        if args.contains(&"--help") || args.first() == Some(&"help") {
            assert!(text.contains("Examples:"), "{args:?}: {text}");
            assert!(!text.contains("app-stop") && !text.contains("cleanup"));
        }
        assert_only_styles(&text);
    }
}

#[test]
fn plain_reports_keep_the_existing_format_and_streams() {
    let cli = CliTest::new();
    let status = cli.pipe(&["status"]);
    assert!(status.status.success());
    assert!(status.stderr.is_empty());
    let text = String::from_utf8(status.stdout).unwrap();
    assert_eq!(text, format!("config: {}\ndm: disabled\nlock: disabled\nidle: disabled (service: unavailable, timeout: 300s, inhibitors: honored)\nactive lock: none for uid {}\nwayland: unavailable\n",
        cli.root.join("config.toml").display(), rsdm_infra::lock_control::current_uid()));
    let validate = cli.pipe(&["validate-config"]);
    assert!(validate.status.success());
    assert!(validate.stderr.is_empty());
    assert_eq!(String::from_utf8(validate.stdout).unwrap(), format!("configuration is valid: {}\n", cli.root.join("config.toml").display()));
}

#[test]
fn no_color_dumb_and_very_narrow_terminals_use_plain_text() {
    let cli = CliTest::new();
    for args in [&["--help"][..], &["status"], &["validate-config"]] {
        let mut no_color = cli.command(args);
        no_color.env("NO_COLOR", "1");
        let mut dumb = cli.command(args);
        dumb.env("TERM", "dumb");
        for (command, width) in [(no_color, 80), (dumb, 80), (cli.command(args), 20)] {
            let (status, text) = terminal(command, width);
            assert!(status.success());
            assert!(!text.contains('\x1b') && !text.contains('╭'), "{text}");
        }
    }
}

#[test]
fn usage_and_runtime_errors_keep_failure_exit_codes() {
    let cli = CliTest::new();
    let args = cli.pipe(&["invalid-command"]);
    assert_eq!(args.status.code(), Some(2));
    assert!(args.stdout.is_empty());
    assert!(String::from_utf8(args.stderr).unwrap().contains("error:"));

    fs::write(cli.root.join("config.toml"), "invalid = [").unwrap();
    let runtime = cli.pipe(&["validate-config"]);
    assert_eq!(runtime.status.code(), Some(1));
    assert!(runtime.stdout.is_empty());
    assert!(!runtime.stderr.contains(&0x1b));
    let (status, text) = terminal(cli.command(&["validate-config"]), 80);
    assert_eq!(status.code(), Some(1));
    assert!(text.contains("COMMAND FAILED"));
    assert_only_styles(&text);
}

#[test]
fn warnings_stay_on_stderr_and_in_the_log_file() {
    let cli = CliTest::new();
    let file = cli.root.join("rsdm.log");
    fs::write(cli.root.join("config.toml"), format!("[dm]\nenable = false\n[logging]\nfile = {:?}\n", file.to_str().unwrap())).unwrap();
    let result = cli.pipe(&["validate-config"]);
    assert!(result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.starts_with("configuration is valid:"));
    assert!(!stdout.contains("warning:"));
    assert!(stderr.contains("warning:"), "{stderr}");
    assert!(fs::read_to_string(&file).unwrap().contains("config warning"));

    let (status, text) = terminal(cli.command(&["validate-config"]), 80);
    assert!(status.success());
    assert!(text.contains("CONFIGURATION WARNING"));
    assert!(!text.contains("config warning field="), "{text}");
    assert_only_styles(&text);
}
