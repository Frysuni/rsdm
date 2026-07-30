use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use rsdm_core::domain::{LoggingConfig, LoggingLevel};
use rsdm_infra::config::load_config;
use tracing_subscriber::{
    EnvFilter,
    fmt::writer::{BoxMakeWriter, MakeWriterExt},
};

pub fn init(config_path: &Path) {
    let (logging, load_error) = match load_config(config_path) {
        Ok(config) => (config.logging, None),
        Err(error) => (LoggingConfig::default(), Some(error.to_string())),
    };
    let filter = logging_filter(&logging);
    let (writer, destination) = logging_writer(&logging);

    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .try_init()
    {
        eprintln!("warning: failed to initialize logging: {error}");
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
        eprintln!("warning: invalid RUST_LOG; falling back to config level: {error}");
        EnvFilter::new(default_filter(logging.level))
    })
}

fn default_filter(level: LoggingLevel) -> String {
    let level = level.as_str();
    format!(
        "warn,rsdm={level},rsdm_core={level},rsdm_idle={level},rsdm_infra={level},rsdm_lock={level},rsdm_tui={level}"
    )
}

fn logging_writer(logging: &LoggingConfig) -> (BoxMakeWriter, String) {
    let journald = "journald".to_string();
    let Some(path) = logging
        .file
        .as_deref()
        .filter(|file| !file.trim().is_empty())
        .map(PathBuf::from)
    else {
        return (BoxMakeWriter::new(io::stderr), journald);
    };

    match open_log_file(&path) {
        Ok(file) => (
            BoxMakeWriter::new(io::stderr.and(Mutex::new(file))),
            format!("journald + {}", path.display()),
        ),
        Err(error) => {
            eprintln!(
                "warning: failed to open log file {}; logging to the journal only: {error}",
                path.display()
            );
            (BoxMakeWriter::new(io::stderr), journald)
        }
    }
}

fn open_log_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o640);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn default_filter_keeps_dependency_noise_down() {
        assert_eq!(
            default_filter(LoggingLevel::Debug),
            "warn,rsdm=debug,rsdm_core=debug,rsdm_idle=debug,rsdm_infra=debug,rsdm_lock=debug,rsdm_tui=debug"
        );
    }

    #[test]
    fn log_file_creates_parent_directory_and_appends() {
        let dir = std::env::temp_dir().join(format!("rsdm-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("rsdm.log");

        {
            let mut file = open_log_file(&path).expect("open log file");
            writeln!(file, "first").expect("write first line");
        }
        {
            let mut file = open_log_file(&path).expect("reopen log file");
            writeln!(file, "second").expect("write second line");
        }

        let text = fs::read_to_string(&path).expect("read log file");
        assert!(text.contains("first"));
        assert!(text.contains("second"));
        fs::remove_dir_all(&dir).expect("remove temp log dir");
    }
}
