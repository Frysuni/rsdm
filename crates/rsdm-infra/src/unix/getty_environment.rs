//! Resolve the configured getty's values before command-line expansion.

use std::{collections::BTreeMap, ffi::{CStr, CString}, io, os::unix::ffi::OsStrExt, path::PathBuf};

use zbus::Proxy;

use super::{getty_environment_file, getty_expansion::valid_name};

pub(super) async fn load(manager: &Proxy<'_>, service: &Proxy<'_>) -> Option<BTreeMap<String, String>> {
    let (base, unit, files) = tokio::try_join!(
        manager.get_property::<Vec<String>>("Environment"),
        service.get_property::<Vec<String>>("Environment"),
        service.get_property::<Vec<(String, bool)>>("EnvironmentFiles"),
    ).ok()?;
    let mut environment = assignments(base);
    // Fallback owns a Linux virtual console, independently of the DM's TERM.
    environment.entry("TERM".into()).or_insert_with(|| "linux".into());
    environment.extend(assignments(unit));
    tokio::task::spawn_blocking(move || load_files(environment, &files)).await.ok()?.ok()
}

fn assignments(values: Vec<String>) -> BTreeMap<String, String> {
    values.into_iter().filter_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        valid_name(name).then(|| (name.to_string(), value.to_string()))
    }).collect()
}

fn load_files(mut environment: BTreeMap<String, String>, files: &[(String, bool)]) -> io::Result<BTreeMap<String, String>> {
    for (pattern, optional) in files {
        let paths = match expand_paths(pattern) {
            Ok(paths) => paths,
            Err(_) if *optional => continue,
            Err(error) => return Err(error),
        };
        for path in paths {
            match std::fs::read_to_string(&path).and_then(|text| getty_environment_file::parse(&text)) {
                Ok(values) => environment.extend(values),
                Err(_) if *optional => {},
                Err(error) => return Err(error),
            }
        }
    }
    Ok(environment)
}

fn expand_paths(pattern: &str) -> io::Result<Vec<PathBuf>> {
    let pattern = CString::new(pattern).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut matches = std::mem::MaybeUninit::<libc::glob_t>::zeroed();
    // SAFETY: pattern is terminated, matches is writable, and no callback is used.
    let result = unsafe { libc::glob(pattern.as_ptr(), 0, None, matches.as_mut_ptr()) };
    // SAFETY: glob initialized the zeroed result, including partial allocations on error.
    let mut matches = unsafe { matches.assume_init() };
    let paths = if result == 0 {
        (0..matches.gl_pathc).map(|index| {
            // SAFETY: successful glob owns gl_pathc terminated path strings.
            let bytes = unsafe { CStr::from_ptr(*matches.gl_pathv.add(index)) }.to_bytes();
            PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
        }).collect()
    } else { Vec::new() };
    // SAFETY: free exactly the allocations owned by this glob result.
    unsafe { libc::globfree(&mut matches); }
    match result {
        0 => Ok(paths),
        libc::GLOB_NOMATCH => Err(io::Error::from(io::ErrorKind::NotFound)),
        _ => Err(io::Error::other("cannot enumerate getty environment files")),
    }
}

#[cfg(test)]
#[path = "getty_environment_tests.rs"]
mod tests;
