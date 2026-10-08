use std::{ffi::CString, os::raw::c_char};

use rsdm_core::ports::SessionLaunchError;

pub struct PreparedCommand {
    pub program: CString,
    argv: Vec<CString>,
}

impl PreparedCommand {
    /// Build a command whose argv is `prefix` followed by the split `exec`.
    /// `program` is the first element of the combined argv, so an empty prefix
    /// runs the session directly while a prefix like `[rsdm, session, start,
    /// --]` wraps it in the session manager.
    #[cfg(test)]
    pub fn new_wrapped(prefix: &[String], exec: &str) -> Result<Self, SessionLaunchError> {
        Self::new_wrapped_argv(prefix, &split_exec(exec)?)
    }

    pub fn new_wrapped_argv(prefix: &[String], session_argv: &[String]) -> Result<Self, SessionLaunchError> {
        if session_argv.is_empty() {
            return Err(SessionLaunchError::Setup("session command is empty".to_string()));
        }
        let mut args = if is_session_manager_command(session_argv) { Vec::new() } else { prefix.to_vec() };
        args.extend_from_slice(session_argv);
        let argv = args
            .iter()
            .enumerate()
            .map(|(index, arg)| cstring(&format!("session argument {index}"), arg))
            .collect::<Result<Vec<_>, _>>()?;
        let program = argv
            .first()
            .ok_or_else(|| SessionLaunchError::Setup("session command is empty".to_string()))?
            .clone();

        Ok(Self { program, argv })
    }

    pub fn argv_ptrs(&self) -> Vec<*const c_char> {
        self.argv
            .iter()
            .map(|arg| arg.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect()
    }
}

pub(super) fn is_session_manager_command(argv: &[String]) -> bool {
    let rsdm = argv.first().is_some_and(|program| std::path::Path::new(program).file_name().is_some_and(|name| name == "rsdm"));
    if !rsdm { return false; }
    let mut arguments = argv.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--config" { arguments.next(); continue; }
        if argument.starts_with("--config=") { continue; }
        return argument == "session" && arguments.next().is_some_and(|action| action == "start");
    }
    false
}

pub fn cstring(label: &str, value: &str) -> Result<CString, SessionLaunchError> {
    CString::new(value).map_err(|_| SessionLaunchError::Setup(format!("{label} contains NUL byte")))
}

pub fn split_exec(input: &str) -> Result<Vec<String>, SessionLaunchError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut arg_started = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            arg_started = true;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            in_quotes = !in_quotes;
            arg_started = true;
        } else if ch.is_whitespace() && !in_quotes {
            push_arg(&mut args, &mut current, &mut arg_started);
        } else {
            current.push(ch);
            arg_started = true;
        }
    }

    if escaped {
        return Err(SessionLaunchError::Setup(
            "session command has a dangling escape".to_string(),
        ));
    }
    if in_quotes {
        return Err(SessionLaunchError::Setup(
            "session command has an unterminated quote".to_string(),
        ));
    }
    push_arg(&mut args, &mut current, &mut arg_started);

    if args.first().is_none_or(String::is_empty) {
        Err(SessionLaunchError::Setup(
            "session command is empty".to_string(),
        ))
    } else {
        Ok(args)
    }
}

fn push_arg(args: &mut Vec<String>, current: &mut String, arg_started: &mut bool) {
    if *arg_started {
        args.push(std::mem::take(current));
        *arg_started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_strings(command: &PreparedCommand) -> Vec<String> {
        command
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn splits_simple_command() {
        let command = PreparedCommand::new_wrapped(&[], "niri --session").unwrap();
        assert_eq!(argv_strings(&command), vec!["niri", "--session"]);
        assert_eq!(command.program.to_string_lossy(), "niri");
    }

    #[test]
    fn honours_quotes_and_escapes() {
        let command = PreparedCommand::new_wrapped(&[], r#"sh -c "echo hi""#).unwrap();
        assert_eq!(argv_strings(&command), vec!["sh", "-c", "echo hi"]);
    }

    #[test]
    fn rejects_unterminated_quote() {
        assert!(PreparedCommand::new_wrapped(&[], "sh -c \"oops").is_err());
    }

    #[test]
    fn rejects_empty_command() {
        assert!(PreparedCommand::new_wrapped(&[], "   ").is_err());
        assert!(PreparedCommand::new_wrapped(&[], r#""" --session"#).is_err());
        assert!(PreparedCommand::new_wrapped(&["wrapper".into()], r#""""#).is_err());
    }

    #[test]
    fn preserves_empty_arguments_after_the_program() {
        let command = PreparedCommand::new_wrapped(&[], r#"program "" value"#).unwrap();
        assert_eq!(argv_strings(&command), ["program", "", "value"]);
    }

    #[test]
    fn prefix_wraps_the_session_command() {
        let prefix = [
            "rsdm".to_string(),
            "session".to_string(),
            "start".to_string(),
            "--".to_string(),
        ];
        let command = PreparedCommand::new_wrapped(&prefix, "niri-session").unwrap();
        assert_eq!(
            argv_strings(&command),
            vec!["rsdm", "session", "start", "--", "niri-session"]
        );
        assert_eq!(command.program.to_string_lossy(), "rsdm");
    }

    #[test]
    fn explicit_session_start_is_not_wrapped_twice() {
        let prefix = vec!["/usr/bin/rsdm".into(), "session".into(), "start".into(), "--".into()];
        let command = PreparedCommand::new_wrapped(&prefix, "/usr/bin/rsdm --config /etc/example.toml session start -- example-session").unwrap();
        assert_eq!(argv_strings(&command), ["/usr/bin/rsdm", "--config", "/etc/example.toml", "session", "start", "--", "example-session"]);
        let command = PreparedCommand::new_wrapped(&prefix, "example -- session start").unwrap();
        assert_eq!(&argv_strings(&command)[..4], prefix.as_slice());
        assert!(!is_session_manager_command(&split_exec("rsdm app -- example session start").unwrap()));
    }

    #[test]
    fn expanded_desktop_arguments_survive_preparation_and_wrapping() {
        let entry = crate::sessions::parse_desktop_entry(
            "[Desktop Entry]\nName=Desktop %k\nIcon=icon %f\nExec=program %c %k %i \"\" %%\n",
        ).unwrap();
        let session = entry.to_session("desktop", "/entries/a \"quoted\" %c.desktop").unwrap();
        let command = PreparedCommand::new_wrapped(&["wrapper".into()], &session.exec).unwrap();
        assert_eq!(argv_strings(&command), ["wrapper", "program", "Desktop %k",
            "/entries/a \"quoted\" %c.desktop", "--icon", "icon %f", "", "%"]);
    }
}
