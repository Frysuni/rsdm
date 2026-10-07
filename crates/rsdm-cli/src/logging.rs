use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use rsdm_core::domain::{LoggingConfig, LoggingLevel};
use rsdm_infra::config::load_config;
use tracing_subscriber::{
    EnvFilter, Layer, fmt::writer::BoxMakeWriter, layer::SubscriberExt, util::SubscriberInitExt,
};

mod file;

pub fn init(config_path: &Path) {
    let (logging, load_error) = match load_config(config_path) {
        Ok(config) => (config.logging, None),
        Err(error) => (LoggingConfig::default(), Some(error.to_string())),
    };
    let filter = logging_filter(&logging);
    let (log_file, destination) = open_log_destination(&logging);

    let decorated = crate::output::stderr_is_decorated();
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(BoxMakeWriter::new(io::stderr))
        .with_ansi(false)
        .with_target(true)
        .event_format(crate::output::ConsoleFormat)
        // CLI diagnostics already have a terminal report; retain their structured file/journal events.
        .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
            !decorated || metadata.target() != "rsdm::cli"
        }));
    let init_result = if let Some(file) = log_file {
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(BoxMakeWriter::new(Mutex::new(file)))
            .with_ansi(false)
            .with_target(true);

        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .with(file_layer)
            .try_init()
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .try_init()
    };

    if let Err(error) = init_result {
        let _ = crate::output::notice("LOGGING WARNING", format!("warning: failed to initialize logging: {error}\n"), crate::output::WARNING).stderr();
        return;
    }

    if let Some(error) = load_error {
        tracing::warn!(
            config = %config_path.display(),
            %error,
            "failed to load config for logging; using built-in logging defaults"
        );
    }
    tracing::debug!(
        config = %config_path.display(),
        destination,
        level = logging.level.as_str(),
        rust_log_override = std::env::var_os("RUST_LOG").is_some(),
        "logging initialized"
    );
}

fn logging_filter(logging: &LoggingConfig) -> EnvFilter {
    if std::env::var_os("RUST_LOG").is_none() {
        return EnvFilter::new(default_filter(logging.level));
    }
    EnvFilter::try_from_default_env().unwrap_or_else(|error| {
        let _ = crate::output::notice("LOGGING WARNING", format!("warning: invalid RUST_LOG; falling back to config level: {error}\n"), crate::output::WARNING).stderr();
        EnvFilter::new(default_filter(logging.level))
    })
}

fn default_filter(level: LoggingLevel) -> String {
    let level = level.as_str();
    format!(
        "warn,rsdm={level},rsdm_core={level},rsdm_idle={level},rsdm_infra={level},rsdm_lock={level},rsdm_tui={level}"
    )
}

fn open_log_destination(logging: &LoggingConfig) -> (Option<File>, String) {
    let journald = "journald".to_string();
    let Some(path) = logging
        .file
        .as_deref()
        .filter(|file| !file.trim().is_empty())
        .map(PathBuf::from)
    else {
        return (None, journald);
    };

    match file::open(&path) {
        Ok(file) => (Some(file), format!("journald + {}", path.display())),
        Err(error) => {
            let _ = crate::output::notice("LOGGING WARNING", format!(
                "warning: failed to open log file {}; logging to the journal only: {error}\n",
                path.display()
            ), crate::output::WARNING).stderr();
            (None, journald)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_keeps_dependency_noise_down() {
        assert_eq!(
            default_filter(LoggingLevel::Debug),
            "warn,rsdm=debug,rsdm_core=debug,rsdm_idle=debug,rsdm_infra=debug,rsdm_lock=debug,rsdm_tui=debug"
        );
    }
}
