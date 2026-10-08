use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub(crate) struct AtomicWriteOptions {
    pub(crate) mode: u32,
    pub(crate) sync_file: bool,
    pub(crate) sync_parent: bool,
}

pub(crate) fn write(
    path: &Path,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let (temporary, mut file) = create_temp(path, options.mode)?;
    let result = (|| {
        file.write_all(bytes)?;
        if options.sync_file {
            file.sync_all()?;
        }
        drop(file);
        fs::rename(&temporary, path)?;
        if options.sync_parent {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn create_temp(path: &Path, mode: u32) -> io::Result<(PathBuf, File)> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);

    for attempt in 0..16 {
        let temporary = parent.join(format!(
            ".{name}.rsdm-{}-{nanos}-{}-{}.tmp",
            std::process::id(),
            counter,
            attempt
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options
            .mode(mode & 0o777)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique atomic replacement file",
    ))
}
