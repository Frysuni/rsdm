use std::{
    io::{BufRead, BufReader}, os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, Command, Stdio}, thread,
};

use anyhow::{Context, Result};

use crate::{LogComponent, output::{self, ACCENT, MUTED, Report, WARNING}};

pub fn run(follow: bool, lines: u32, component: LogComponent) -> Result<()> {
    let mut command = command(follow, lines, component);
    if !output::stdout_is_decorated() {
        return Err(command.exec()).context("opening rsdm journal with journalctl");
    }
    command.args(["--output=json", "--all", "--no-pager"])
        .env("SYSTEMD_COLORS", "0").stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut process = JournalProcess(command.spawn().context("opening rsdm journal with journalctl")?);
    let stdout = process.0.stdout.take().expect("piped journal stdout");
    let stderr = process.0.stderr.take().expect("piped journal stderr");
    header(follow, lines, component)?;
    let diagnostics = thread::spawn(move || -> Result<()> {
        for line in BufReader::new(stderr).lines() {
            output::notice("JOURNAL MESSAGE", format!("{}\n", line?), WARNING).stderr()?;
        }
        Ok(())
    });
    let result = display(BufReader::new(stdout));
    if result.is_err() { let _ = process.0.kill(); }
    let status = process.0.wait().context("waiting for journalctl")?;
    diagnostics.join().map_err(|_| anyhow::anyhow!("journal diagnostic reader failed"))??;
    let count = result?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(1)));
    }
    if count == 0 {
        output::notice("JOURNAL EMPTY", "No matching journal entries.\n", MUTED).stdout()?;
    }
    Ok(())
}

fn command(follow: bool, lines: u32, component: LogComponent) -> Command {
    let mut command = Command::new("journalctl");
    command.args(["--boot", "--output=short-precise", "--no-hostname", "--pager-end"])
        .arg(format!("--lines={lines}"));
    if follow { command.arg("--follow"); }
    match component {
        LogComponent::All => { command.arg("SYSLOG_IDENTIFIER=rsdm"); }
        LogComponent::Dm => { command.arg("--unit=rsdm.service"); }
        LogComponent::Idle => { command.arg("--user-unit=rsdm-idle.service"); }
        LogComponent::Lock => {
            command.args(["SYSLOG_IDENTIFIER=rsdm", "--grep=rsdm_lock|lock screen|session lock|idle locker"]);
        }
    }
    command
}

fn header(follow: bool, lines: u32, component: LogComponent) -> Result<()> {
    let mut report = Report::new("JOURNAL", ACCENT);
    let component = match component {
        LogComponent::All => "DM / Greeter / Lock / Idle",
        LogComponent::Dm => "DM / Greeter", LogComponent::Idle => "Idle", LogComponent::Lock => "Lock",
    };
    report.field("component", component, ACCENT);
    report.field("mode", if follow { "live stream · Ctrl+C to stop" } else { "recent entries" }, ACCENT);
    report.field("limit", format!("{lines} entries from the current boot"), MUTED);
    report.field("time", "UTC", MUTED);
    report.stdout()?;
    Ok(())
}

fn display(reader: impl BufRead) -> Result<usize> {
    let mut count = 0;
    for line in reader.lines() {
        output::journal_record(&line.context("reading journal entry")?)?;
        count += 1;
    }
    Ok(count)
}

struct JournalProcess(Child);

impl Drop for JournalProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
