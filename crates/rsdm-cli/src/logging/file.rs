//! Append-only logging through pinned, trusted parent directories.

use std::{
    ffi::CString,
    fs::{File, Metadata},
    io,
    os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::MetadataExt}},
    path::{Component, Path},
};

pub(super) fn open(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "log path must be absolute"));
    }
    let name = path.file_name().ok_or_else(|| unsafe_path("log path has no filename"))?;
    let name = CString::new(name.as_bytes()).map_err(|_| unsafe_path("log filename contains NUL"))?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let parent = open_parent(path.parent().expect("absolute file has a parent"), uid)?;
    // SAFETY: parent is an owned directory FD and name is a valid C string.
    let fd = unsafe {
        libc::openat(parent.as_raw_fd(), name.as_ptr(),
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_NOFOLLOW
                | libc::O_CLOEXEC | libc::O_NONBLOCK, 0o640 as libc::mode_t)
    };
    let file = owned_file(fd)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(unsafe_path("log must be a regular file owned by this user without shared write access"));
    }
    Ok(file)
}

fn open_parent(path: &Path, uid: u32) -> io::Result<File> {
    // SAFETY: the literal is NUL-terminated; successful open returns a new FD.
    let mut parent = owned_file(unsafe {
        libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
    })?;
    validate_directory(&parent.metadata()?, uid, true)?;
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => CString::new(name.as_bytes())
                .map_err(|_| unsafe_path("log directory contains NUL"))?,
            _ => return Err(unsafe_path("log path contains an unsupported component")),
        };
        parent = open_directory(&parent, &name)?;
        validate_directory(&parent.metadata()?, uid, true)?;
    }
    validate_directory(&parent.metadata()?, uid, false)?;
    Ok(parent)
}

fn open_directory(parent: &File, name: &CString) -> io::Result<File> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: parent is a live directory FD and name is a valid C string.
    let mut fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound { return Err(error); }
        // SAFETY: create a child of the pinned directory, without following a path.
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o750) };
        if created < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists { return Err(error); }
        }
        // A concurrent creator still has to pass the descriptor checks below.
        // SAFETY: same pinned directory, name, and open flags as above.
        fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    }
    owned_file(fd)
}

fn validate_directory(metadata: &Metadata, uid: u32, ancestor: bool) -> io::Result<()> {
    if !metadata.is_dir() || (metadata.uid() != 0 && metadata.uid() != uid) {
        return Err(unsafe_path("log directory is not owned by root or this user"));
    }
    // Trusted sticky ancestors such as /tmp are safe for traversing into a
    // private owned directory, but must never be the immediate log directory.
    if metadata.mode() & 0o022 != 0 && !(ancestor && metadata.mode() & 0o1000 != 0) {
        return Err(unsafe_path("log directory permits shared writes"));
    }
    Ok(())
}

fn owned_file(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: the successful open/openat returned a descriptor we now own.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn unsafe_path(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
