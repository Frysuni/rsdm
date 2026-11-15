//! Animated terminal view; power decisions stay outside the renderer.

use std::{io::{self, Stdout}, time::Instant};

use ratatui::{
    Frame, Terminal, backend::CrosstermBackend,
    crossterm::{execute, terminal::{EnterAlternateScreen, LeaveAlternateScreen}},
    layout::{Constraint, Layout}, style::Style, text::{Line, Span},
    widgets::{Block, BorderType, Gauge, Paragraph},
};

use crate::power_state::Progress;

use super::{ACCENT, BORDER, MUTED, Report, SECONDARY, SUCCESS, WARNING, safe_text, state_color};

pub(crate) struct PowerScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl PowerScreen {
    pub fn open() -> io::Result<Self> {
        let mut screen = Self { terminal: Terminal::new(CrosstermBackend::new(io::stdout()))? };
        execute!(screen.terminal.backend_mut(), EnterAlternateScreen)?;
        screen.terminal.clear()?;
        Ok(screen)
    }

    pub fn draw(&mut self, action: &str, progress: &Progress) -> io::Result<()> {
        self.terminal.draw(|frame| render(frame, action, progress))?;
        Ok(())
    }
}

impl Drop for PowerScreen {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

pub(crate) fn summary(progress: &Progress) -> Report {
    let mut report = Report::new("POWER PROGRESS", ACCENT);
    report.field("elapsed", format!("{}s", progress.started.elapsed().as_secs()), ACCENT);
    let mut rows = status_rows(progress);
    if progress.observed_shutdown && !progress.apps.is_empty() {
        rows.push(section("APPLICATION PREPARATION RESULTS"));
        application_rows(progress, &mut rows);
    }
    let plain = rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n";
    report.message(plain, rows);
    report
}

fn render(frame: &mut Frame<'_>, action: &str, progress: &Progress) {
    let title = if action == "reboot" { "REBOOT" } else { "POWER OFF" };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(BORDER))
        .title(Line::styled(format!(" RSDM / {title} "), Style::new().fg(ACCENT).bold()));
    let area = block.inner(frame.area());
    frame.render_widget(block, frame.area());
    let [header, gauge, body, footer] = Layout::vertical([
        Constraint::Length(2), Constraint::Length(3), Constraint::Min(0), Constraint::Length(4),
    ]).margin(1).areas(area);
    let spinner = spinner(progress);
    frame.render_widget(Paragraph::new(header_rows(progress)), header);

    let stage = progress.stage();
    let stages = ["Applications", "Power request / session teardown", "System services"];
    let prepared = progress.apps.values().filter(|app| app.finished()).count();
    let app_fraction = if stage == 0 && !progress.apps.is_empty() { prepared as f64 / progress.apps.len() as f64 } else { 0.0 };
    frame.render_widget(Gauge::default().block(Block::bordered().border_style(Style::new().fg(BORDER)))
        .gauge_style(Style::new().fg(ACCENT)).ratio((stage as f64 + app_fraction) / 3.0)
        .label(format!("{spinner} Stage {}/3 · {}", stage + 1, stages[stage])), gauge);

    let mut rows = status_rows(progress);
    let available = usize::from(body.height);
    if rows.len() > available && available > 0 {
        let hidden = rows.len() - available + 1;
        rows.truncate(available - 1);
        rows.push(Line::styled(format!("… {hidden} more status rows; oldest active waits shown first"), Style::new().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(rows), body);

    frame.render_widget(Paragraph::new(vec![
        Line::styled("Progress covers observed work; overall shutdown time is unknown.", Style::new().fg(MUTED)),
        Line::styled("The display ends when this terminal or the system bus closes.", Style::new().fg(MUTED)),
        Line::styled("Ctrl+C: stop watching. A dispatched power request may continue.", Style::new().fg(WARNING)),
        Line::styled(progress.outcome.as_deref().map(safe_text).unwrap_or_default(), Style::new().fg(SECONDARY)),
    ]), footer);
}

fn header_rows(progress: &Progress) -> Vec<Line<'static>> {
    let phase = if progress.observed_shutdown { "Stopping system services" } else {
        match progress.phase.as_str() {
            "running" => "Waiting for authorization / session coordinator",
            "preparing" => "Waiting for applications to close",
            "stopping_session" => "Waiting for session teardown / power handoff",
            "closed" => "Waiting for system shutdown",
            "inactive" => "Session coordinator is no longer available",
            phase => phase,
        }
    };
    vec![
        Line::styled(format!("{}  {}   ·   {}s elapsed", spinner(progress), safe_text(phase), progress.started.elapsed().as_secs()), Style::new().fg(ACCENT).bold()),
        Line::styled(format!("Session provider: {}", safe_text(&progress.provider)), Style::new().fg(MUTED)),
    ]
}

fn status_rows(progress: &Progress) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    if let Some(error) = &progress.session_error {
        rows.push(Line::styled(format!("Session status unavailable: {}", safe_text(error)), Style::new().fg(WARNING)));
    }
    for error in &progress.system_errors {
        rows.push(Line::styled(safe_text(error), Style::new().fg(WARNING)));
    }
    let done = progress.apps.values().filter(|app| app.finished()).count();
    rows.push(section(format!("APPLICATIONS · {done}/{} reported prepared", progress.apps.len())));
    if !progress.observed_shutdown {
        application_rows(progress, &mut rows);
    }
    if let Some(status) = &progress.logind {
        rows.push(section(if status.preparing { "LOGIND · shutdown announced" } else { "LOGIND · awaiting shutdown announcement" }));
        for inhibitor in &status.inhibitors {
            let label = if status.preparing && inhibitor.mode == "delay" { "delay holder" } else { "inhibitor" };
            rows.push(Line::from(vec![
                Span::styled(format!("{} {label} ", spinner(progress)), Style::new().fg(WARNING)),
                Span::raw(format!("{} (PID {}, {}): {}", safe_text(&inhibitor.who), inhibitor.pid,
                    safe_text(&inhibitor.mode), safe_text(&inhibitor.why))),
            ]));
        }
        if status.inhibitors.is_empty() { rows.push(Line::raw("No shutdown inhibitors reported")); }
    }
    if progress.observed_shutdown { system_rows(progress, &mut rows); }
    rows
}

fn application_rows(progress: &Progress, rows: &mut Vec<Line<'static>>) {
    let mut apps: Vec<_> = progress.apps.iter().collect();
    apps.sort_by_key(|(_, app)| (app.ended.is_some(), app.waiting.is_none(), app.waiting));
    for (unit, app) in apps {
        let symbol = if app.finished() { "✓" } else { spinner(progress) };
        let color = if app.state == "clean" { SUCCESS } else if app.state == "forced" { WARNING } else { state_color(&app.state) };
        let until = app.ended.unwrap_or_else(Instant::now);
        let elapsed = app.waiting.map_or(0, |since| until.duration_since(since).as_secs());
        let methods = if app.methods_used.is_empty() { format!("policy {}", app.method) } else { app.methods_used.join(" → ") };
        rows.push(Line::from(vec![
            Span::styled(format!("{symbol} {} ", safe_text(&app.state)), Style::new().fg(color)),
            Span::raw(safe_text(unit)),
            Span::styled(format!(" · {} · observed {elapsed}s · timeout {}s", safe_text(&methods), app.timeout), Style::new().fg(MUTED)),
        ]));
    }
    if progress.apps.is_empty() { rows.push(Line::raw("No registered application work reported")); }
}

fn system_rows(progress: &Progress, rows: &mut Vec<Line<'static>>) {
    let mut jobs: Vec<_> = progress.jobs.values().collect();
    jobs.sort_by_key(|job| (!job.queued, job.job.state != "running", job.since));
    let queued = jobs.iter().filter(|job| job.queued).count();
    rows.push(section(format!("SYSTEMD · {queued} queued · {} observed jobs left the queue", jobs.len() - queued)));
    for tracked in jobs {
        let job = &tracked.job;
        let (symbol, state, color) = if tracked.queued { (spinner(progress), job.state.as_str(), WARNING) } else { ("·", "left queue", MUTED) };
        let elapsed = tracked.ended.unwrap_or_else(Instant::now).duration_since(tracked.since).as_secs();
        rows.push(Line::from(vec![
            Span::styled(format!("{symbol} {} ", safe_text(state)), Style::new().fg(color)),
            Span::raw(safe_text(&job.unit)),
            Span::styled(format!(" · {} · observed {elapsed}s", safe_text(&job.kind)), Style::new().fg(MUTED)),
        ]));
    }
    rows.push(Line::styled("Running jobs are executing; waiting jobs await systemd ordering.", Style::new().fg(MUTED)));
    rows.push(Line::styled("Leaving the queue does not confirm success; fast jobs may be missed.", Style::new().fg(MUTED)));
}

fn spinner(progress: &Progress) -> &'static str {
    const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
    FRAMES[(progress.started.elapsed().as_millis() / 100 % FRAMES.len() as u128) as usize]
}

fn section(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), Style::new().fg(SECONDARY).bold())
}

#[cfg(test)]
#[path = "power_tests.rs"]
mod tests;
