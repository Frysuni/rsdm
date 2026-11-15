use std::time::Duration;

use ratatui::backend::TestBackend;
use rsdm_system::shutdown::{Inhibitor, Job, LogindStatus};

use super::*;

fn view(progress: &Progress, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, "reboot", progress)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>().join("\n")
}

#[test]
fn system_waits_show_methods_reasons_elapsed_time_and_honest_progress() {
    let mut progress = Progress::new();
    progress.system(Ok(vec![Job { id: 1, unit: "slow.service".into(), kind: "stop".into(), state: "running".into() }]),
        Ok(LogindStatus { preparing: true, inhibitors: vec![Inhibitor {
            who: "Editor".into(), why: "Saving document".into(), mode: "delay".into(), pid: 42,
        }] }));
    let text = view(&progress, 100, 28);
    for expected in ["REBOOT", "Stage 3/3", "slow.service", "stop", "running", "observed 0s", "Saving document", "PID 42", "time is unknown"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("100%"));
}

#[test]
fn control_characters_are_escaped_and_small_terminals_do_not_panic() {
    let mut progress = Progress::new();
    progress.session_error = Some("untrusted\x1b[2J\rtext".into());
    let text = view(&progress, 100, 24);
    assert!(text.contains("untrusted\\u{1b}[2J\\rtext"));
    assert!(!text.contains('\x1b'));
    for size in [(28, 8), (1, 1), (0, 0)] { view(&progress, size.0, size.1); }
}

#[test]
fn animation_changes_without_another_diagnostic_snapshot() {
    let mut progress = Progress::new();
    let before = view(&progress, 80, 24);
    progress.started -= Duration::from_millis(100);
    assert_ne!(before, view(&progress, 80, 24));
}
