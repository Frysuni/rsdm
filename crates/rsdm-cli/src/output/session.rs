use std::io;

use ratatui::{style::Style, text::{Line, Span}};
use rsdm_core::domain::ShutdownPolicy;
use rsdm_infra::session_manager::SessionStatus;

use super::{ACCENT, MUTED, Report, SUCCESS, WARNING, safe_text, state_color};

pub fn session_status(status: &SessionStatus) -> io::Result<()> {
    let mut report = Report::new("SESSION STATUS", ACCENT);
    let summary = format!("{}: {} (login {}, desktop {}, generation {})\n", status.provider,
        status.phase, status.login_session_id, status.desktop_entry_id, status.generation);
    report.message(summary, vec![Line::from(vec![
        Span::styled(format!("● {}", safe_text(&status.phase)), Style::new().fg(state_color(&status.phase)).bold()),
        Span::styled(format!("  /  {}", safe_text(&status.provider)), Style::new().fg(ACCENT)),
    ])]);
    // The plain status format is kept intact for pipes and existing callers.
    let details = [
        ("login", status.login_session_id.as_str()),
        ("desktop", status.desktop_entry_id.as_str()),
        ("generation", status.generation.as_str()),
    ];
    for (label, value) in details {
        report.message("", vec![Line::from(vec![
            Span::styled(format!("{label:<13} "), Style::new().fg(MUTED)),
            Span::raw(safe_text(value)),
        ])]);
    }
    let xsmp = if status.xsmp_available { "available" } else { "unavailable; auto uses the provider's shutdown method" };
    report.field("RSDM XSMP", xsmp, if status.xsmp_available { SUCCESS } else { WARNING });
    report.section(format!("APPLICATIONS · {}", status.apps.len()));
    if status.apps.is_empty() {
        report.message("", vec![Line::styled("No registered applications", Style::new().fg(MUTED))]);
    }
    for (unit, method, timeout, state) in &status.apps {
        report.message(format!("{unit}: {state}, {method}, {timeout}s\n"), vec![
            Line::styled(safe_text(unit), Style::new().fg(ACCENT).bold()),
            Line::from(vec![
                Span::styled(format!("  ● {}", safe_text(state)), Style::new().fg(state_color(state))),
                Span::styled("   method ", Style::new().fg(MUTED)),
                Span::raw(safe_text(method)),
                Span::styled("   timeout ", Style::new().fg(MUTED)),
                Span::raw(format!("{timeout}s")),
            ]),
        ]);
    }
    report.stdout()
}

pub fn application(argv: &[String], policy: &ShutdownPolicy) -> io::Result<()> {
    let mut report = Report::new("APPLICATION LAUNCHED", SUCCESS);
    report.field("program", &argv[0], ACCENT);
    if std::env::var_os("RSDM_SESSION_GENERATION").is_none() {
        report.field("shutdown", "not registered with an RSDM coordinator", WARNING);
        return report.interactive_stdout();
    }
    report.field("timeout", format!("{}s", policy.timeout_secs), ACCENT);
    report.field("on timeout", policy.on_timeout.to_string(), WARNING);
    let method = if policy.quit_command.is_empty() { policy.method.to_string() } else { "quit command".into() };
    report.field("method", method, ACCENT);
    report.interactive_stdout()
}
