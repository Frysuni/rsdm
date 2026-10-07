//! Nonblocking exit checks inside an existing child-process deadline.

use std::{io, process::{Child, ExitStatus}, thread, time::{Duration, Instant}};

pub(super) fn wait_for_exit(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? { return Ok(Some(status)); }
        let now = Instant::now();
        if now >= deadline { return Ok(None); }
        thread::sleep(Duration::from_millis(50).min(deadline - now));
    }
}
