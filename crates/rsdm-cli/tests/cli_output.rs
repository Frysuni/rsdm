//! Verify real terminal output without executing desktop or power actions.

use std::{
    fs, os::unix::fs::PermissionsExt, path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

#[path = "support/terminal.rs"]
mod terminal_output;
use terminal_output::{assert_only_styles, terminal};

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

    fn journal(&self) {
        let script = r#"#!/bin/sh
printf '%s\n' "$@" > "$HOME/journal-args"
case "$*" in
    *--output=json*)
        printf '%s\n' '{"__REALTIME_TIMESTAMP":"1234567","PRIORITY":"4","SYSLOG_IDENTIFIER":"rsdm","MESSAGE":"timeout warning\nsecond line\u001b[2J"}'
        printf '%s\n' '{"__REALTIME_TIMESTAMP":"2234567","PRIORITY":"6","_SYSTEMD_USER_UNIT":"rsdm-idle.service","MESSAGE":[208,159,209,128,208,184,208,178,208,181,209,130]}'
        ;;
    *) printf 'plain journal entry\n' ;;
esac
exit "${JOURNAL_EXIT:-0}"
"#;
        let path = self.root.join("journalctl");
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Drop for CliTest {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
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

#[test]
fn journal_styles_records_and_preserves_component_filters() {
    let cli = CliTest::new();
    cli.journal();
    for (component, filter) in [
        ("all", "SYSLOG_IDENTIFIER=rsdm"), ("dm", "--unit=rsdm.service"),
        ("idle", "--user-unit=rsdm-idle.service"), ("lock", "--grep=rsdm_lock|lock screen|session lock|idle locker"),
    ] {
        let (status, text) = terminal(cli.command(&["logs", "--follow", "--lines", "12", "--component", component]), 80);
        assert!(status.success(), "{text}");
        for expected in ["JOURNAL", "UTC", "WARN", "INFO", "rsdm-idle.service", "timeout warning", "second line", "Привет", "Ctrl+C"] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains("MESSAGE") && !text.contains("PRIORITY"), "{text}");
        assert_only_styles(&text);
        let args = fs::read_to_string(cli.root.join("journal-args")).unwrap();
        for arg in [filter, "--follow", "--lines=12", "--output=json", "--all", "--no-pager", "--boot"] {
            assert!(args.lines().any(|line| line == arg), "{args}");
        }
    }
}

#[test]
fn journal_plain_mode_and_failure_exit_codes_are_preserved() {
    let cli = CliTest::new();
    cli.journal();
    let pipe = cli.pipe(&["logs"]);
    assert!(pipe.status.success());
    assert_eq!(pipe.stdout, b"plain journal entry\n");
    assert!(pipe.stderr.is_empty());
    let mut plain = cli.command(&["logs"]);
    plain.env("NO_COLOR", "1");
    let (status, text) = terminal(plain, 80);
    assert!(status.success());
    assert_eq!(text, "plain journal entry\r\n");
    let args = fs::read_to_string(cli.root.join("journal-args")).unwrap();
    assert!(args.contains("--output=short-precise") && !args.contains("--output=json"));

    let mut failed = cli.command(&["logs"]);
    failed.env("JOURNAL_EXIT", "7");
    let (status, text) = terminal(failed, 80);
    assert_eq!(status.code(), Some(7), "{text}");
    let mut failed = cli.command(&["logs"]);
    failed.env("JOURNAL_EXIT", "7");
    assert_eq!(failed.output().unwrap().status.code(), Some(7));
}

#[test]
fn runtime_diagnostics_share_the_palette_and_preserve_fields() {
    let cli = CliTest::new();
    let mut command = cli.command(&["validate-config"]);
    command.env("RUST_LOG", "rsdm=debug");
    let (status, text) = terminal(command, 80);
    assert!(status.success(), "{text}");
    for expected in ["DEBUG", "rsdm::logging", "logging initialized", "destination=", "level=", "config="] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(text.contains("  │ "), "{text}");
    assert_only_styles(&text);

    let mut command = cli.command(&["validate-config"]);
    command.env("RUST_LOG", "rsdm=debug");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(!output.stderr.contains(&0x1b));
    assert!(String::from_utf8(output.stderr).unwrap().contains("logging initialized"));
}
