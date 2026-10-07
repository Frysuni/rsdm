//! Bound control payloads before they enter the coordinator or recovery state.

use serde::Serialize;
use zbus::zvariant::{LE, Type, serialized::Context, serialized_size};

use super::LaunchRequest;
use crate::session_manager::{env::valid_environment_name, identity::valid_generation};

const MAX_ARGUMENTS: usize = 4096;
const MAX_ENVIRONMENT: usize = 1024;
const MAX_STRING_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_QUIT_ARGUMENTS: usize = 256;
const MAX_QUIT_BYTES: usize = 64 * 1024;

pub(in crate::session_manager) fn validate_launch(request: &LaunchRequest) -> Result<(), String> {
    validate_generation(&request.generation)?;
    strings(&request.argv, MAX_ARGUMENTS)?;
    strings(&request.quit_command, MAX_QUIT_ARGUMENTS)?;
    environment(&request.environment)?;
    for value in [&request.directory, &request.on_timeout, &request.method] {
        string(value)?;
    }
    check_size(&request.quit_command, MAX_QUIT_BYTES)?;
    check_size(request, MAX_REQUEST_BYTES)
}

pub(in crate::session_manager) fn validate_finalize(
    generation: &str, pairs: &[(String, String)],
) -> Result<(), String> {
    validate_generation(generation)?;
    environment(pairs)?;
    check_size(&(generation, pairs), MAX_REQUEST_BYTES)
}

pub(super) fn validate_generation(generation: &str) -> Result<(), String> {
    if !valid_generation(generation) {
        return Err("invalid session generation".into());
    }
    Ok(())
}

pub(super) fn validate_stop(generation: &str, action: &str) -> Result<(), String> {
    validate_generation(generation)?;
    if !matches!(action, "logout" | "reboot" | "poweroff") {
        return Err("invalid session shutdown action".into());
    }
    Ok(())
}

pub(super) fn validate_xsmp(generation: &str, units: &[String]) -> Result<(), String> {
    validate_generation(generation)?;
    strings(units, MAX_ARGUMENTS)?;
    check_size(&(generation, units), MAX_REQUEST_BYTES)
}

fn strings(values: &[String], limit: usize) -> Result<(), String> {
    if values.len() > limit {
        return Err(format!("request exceeds the {limit}-entry argument limit"));
    }
    for value in values {
        string(value)?;
    }
    Ok(())
}

fn environment(pairs: &[(String, String)]) -> Result<(), String> {
    if pairs.len() > MAX_ENVIRONMENT {
        return Err("request exceeds the 1024-entry environment limit".into());
    }
    for (name, value) in pairs {
        string(name)?;
        string(value)?;
        if !valid_environment_name(name) {
            return Err("invalid request environment name".into());
        }
    }
    Ok(())
}

fn string(value: &str) -> Result<(), String> {
    if value.len() > MAX_STRING_BYTES {
        return Err("request string exceeds 64 KiB".into());
    }
    if value.contains('\0') {
        return Err("request strings must not contain NUL".into());
    }
    Ok(())
}

fn check_size<T: Serialize + Type>(value: &T, limit: usize) -> Result<(), String> {
    let size = serialized_size(Context::new_dbus(LE, 0), value)
        .map_err(|error| format!("invalid request encoding: {error}"))?;
    if size.size() > limit {
        return Err(format!("encoded request exceeds {limit} bytes"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "control_payload_tests.rs"]
mod tests;
