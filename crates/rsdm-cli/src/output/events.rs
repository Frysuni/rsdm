//! Terminal formatting for runtime diagnostics; file and journal sinks stay plain.

use std::fmt;

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{FmtContext, FormattedFields, format::{self, FormatEvent, FormatFields}, time::FormatTime},
    registry::LookupSpan,
};

use super::{ACCENT, ERROR, MUTED, WARNING, journal::event_lines, render};

pub struct ConsoleFormat;

impl<S, N> FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: format::Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let Some(width) = render::Stream::Stderr.width() else {
            return format::format().with_ansi(false).format_event(ctx, writer, event);
        };
        let mut timestamp = String::new();
        tracing_subscriber::fmt::time::SystemTime.format_time(&mut format::Writer::new(&mut timestamp))?;
        let metadata = event.metadata();
        let color = match *metadata.level() {
            Level::ERROR => ERROR, Level::WARN => WARNING, Level::INFO => ACCENT, _ => MUTED,
        };
        let message = message(ctx, event)?;
        let lines = event_lines(&timestamp, metadata.level().as_str(), metadata.target(), &message, color);
        let plain = lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n";
        let bytes = render::fragment(lines, width).unwrap_or_else(|| plain.into_bytes());
        writer.write_str(std::str::from_utf8(&bytes).map_err(|_| fmt::Error)?)
    }
}

fn message<S, N>(ctx: &FmtContext<'_, S, N>, event: &Event<'_>) -> Result<String, fmt::Error>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    use std::fmt::Write;

    let mut message = String::new();
    if let Some(scope) = ctx.event_scope() {
        for span in scope.from_root() {
            write!(message, "{}", span.name())?;
            let extensions = span.extensions();
            if let Some(fields) = extensions.get::<FormattedFields<N>>().filter(|fields| !fields.is_empty()) {
                write!(message, "{{{fields}}}")?;
            }
            message.push_str(": ");
        }
    }
    ctx.field_format().format_fields(format::Writer::new(&mut message), event)?;
    Ok(message)
}
