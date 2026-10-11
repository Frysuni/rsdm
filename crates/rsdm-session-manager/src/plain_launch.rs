//! Ordinary commands keep their inherited environment and exit status.

use std::os::unix::process::ExitStatusExt;

use crate::SessionError;

pub(super) async fn run(argv: &[String]) -> Result<i32, SessionError> {
    let program = argv.first().ok_or(SessionError::EmptyCommand)?;
    let mut child = tokio::process::Command::new(program).args(&argv[1..]).spawn()?;
    let status = child.wait().await?;
    Ok(status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1))
}
