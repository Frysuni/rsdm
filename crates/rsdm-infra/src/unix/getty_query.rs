//! Discover the distro's own console login for the TTY fallback.
//!
//! On a systemd box the program that owns a free VT is `getty@<tty>.service`,
//! whose `ExecStart` the distribution bakes in at build time - `agetty` on most
//! systems, but `mingetty`, `busybox getty`, or an agetty at an unusual path on
//! others. Rather than make the operator hardcode that argv in rsdm's config (and
//! get it wrong on a distro that does not ship `agetty`), ask systemd what it
//! already runs and reuse it verbatim.
//!
//! [`discover_system_getty`] runs `systemctl show -p ExecStart` for the VT's
//! getty unit, parses the `argv[]=` field, substitutes the `%I`/`%%` unit
//! specifiers, drops unexpanded `$VAR` environment references (we exec directly,
//! with no shell or systemd to expand them), and returns the resulting argv. It
//! is best-effort: any failure (no systemd, masked unit, unparsable output)
//! returns `None` so the caller falls back to its built-in candidate chain.

use std::process::Command;

/// Resolve the console login systemd runs on `tty_path`, or `None` when it
/// cannot be determined. The result is a ready-to-exec argv.
pub fn discover_system_getty(tty_path: &str) -> Option<Vec<String>> {
    let tty = tty_path.strip_prefix("/dev/").unwrap_or(tty_path);
    // The concrete instance first: its unit may carry distro drop-ins. Fall back
    // to the bare template, whose argv still carries the %I we substitute below.
    for unit in [format!("getty@{tty}.service"), "getty@.service".to_string()] {
        let Some(output) = query_exec_start(&unit) else {
            continue;
        };
        let Some(raw_argv) = extract_argv(&output) else {
            continue;
        };
        let argv = resolve_argv(&raw_argv, tty);
        if !argv.is_empty() {
            tracing::info!(unit, ?argv, "resolved system getty for the TTY fallback");
            return Some(argv);
        }
    }
    None
}

/// Run `systemctl show -p ExecStart <unit>` and return its stdout, or `None` if
/// systemd is missing or the command fails.
fn query_exec_start(unit: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .args(["show", "-p", "ExecStart", "--no-pager", unit])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Pull the `argv[]=...` field out of the `ExecStart={ ... }` line. systemd
/// renders one entry as `ExecStart={ path=/p ; argv[]=/p a b ; flags... }`; the
/// argv runs from `argv[]=` up to the next ` ; ` separator.
fn extract_argv(output: &str) -> Option<String> {
    let start = output.find("argv[]=")? + "argv[]=".len();
    let rest = &output[start..];
    let end = rest.find(" ; ").unwrap_or(rest.len());
    let argv = rest[..end].trim();
    if argv.is_empty() {
        None
    } else {
        Some(argv.to_string())
    }
}

/// Tokenize the argv string, expand unit specifiers, and drop env references.
fn resolve_argv(raw: &str, tty: &str) -> Vec<String> {
    tokenize(raw)
        .into_iter()
        // `$TERM` and friends are expanded by systemd from the unit environment;
        // we exec directly, so a literal `$VAR` would be a bogus argument. Drop
        // them - getty's term type and similar are optional.
        .filter(|token| !is_env_reference(token))
        .map(|token| substitute_specifiers(&token, tty))
        .collect()
}

/// True for a bare environment reference like `$TERM` (but not `$` alone or a
/// value that merely contains a `$`).
fn is_env_reference(token: &str) -> bool {
    let Some(name) = token.strip_prefix('$') else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace the systemd unit specifiers that survive into argv: `%I` is the
/// unescaped instance (our TTY) and `%%` is a literal percent.
fn substitute_specifiers(token: &str, tty: &str) -> String {
    token.replace("%I", tty).replace("%%", "%")
}

/// Split on whitespace, honouring systemd's double quotes (so `-o "-p -- \u"`
/// stays one argument). Only `\"` and `\\` are unescaped; any other backslash is
/// kept verbatim, because sequences like agetty's `\u` are meaningful to the
/// program we are about to exec and must survive intact.
fn tokenize(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut started = false;

    for ch in input.chars() {
        if escaped {
            if ch != '"' && ch != '\\' {
                current.push('\\');
            }
            current.push(ch);
            escaped = false;
            started = true;
        } else if ch == '\\' {
            escaped = true;
            started = true;
        } else if ch == '"' {
            in_quotes = !in_quotes;
            started = true;
        } else if ch.is_whitespace() && !in_quotes {
            if started {
                args.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(ch);
            started = true;
        }
    }
    if escaped {
        // A dangling backslash: keep it rather than silently swallow it.
        current.push('\\');
    }
    if started {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_resolves_a_typical_agetty_unit() {
        let output = "ExecStart={ path=/sbin/agetty ; argv[]=/sbin/agetty -o \"-p -- \\u\" \
             --noclear --keep-baud %I 115200,38400,9600 $TERM ; ignore_errors=yes }";
        let raw = extract_argv(output).unwrap();
        let argv = resolve_argv(&raw, "tty1");
        assert_eq!(
            argv,
            vec![
                "/sbin/agetty",
                "-o",
                "-p -- \\u", // the agetty \u login specifier survives intact
                "--noclear",
                "--keep-baud",
                "tty1",
                "115200,38400,9600",
            ]
        );
        // The %I specifier became the TTY and $TERM was dropped.
        assert!(argv.iter().any(|arg| arg == "tty1"));
        assert!(argv.iter().all(|arg| arg != "$TERM"));
    }

    #[test]
    fn substitutes_template_instance() {
        let raw = "/sbin/agetty --noclear %I linux";
        assert_eq!(
            resolve_argv(raw, "tty3"),
            vec!["/sbin/agetty", "--noclear", "tty3", "linux"]
        );
    }

    #[test]
    fn keeps_mingetty_style_units() {
        let raw = "/sbin/mingetty %I";
        assert_eq!(resolve_argv(raw, "tty2"), vec!["/sbin/mingetty", "tty2"]);
    }

    #[test]
    fn missing_argv_field_yields_none() {
        assert!(extract_argv("ExecStart=").is_none());
        assert!(extract_argv("ExecStart={ path=/sbin/agetty ; argv[]= ; flags }").is_none());
    }

    #[test]
    fn recognizes_env_references() {
        assert!(is_env_reference("$TERM"));
        assert!(is_env_reference("$XDG_VTNR"));
        assert!(!is_env_reference("$"));
        assert!(!is_env_reference("--keep-baud"));
        assert!(!is_env_reference("price$5"));
    }
}
