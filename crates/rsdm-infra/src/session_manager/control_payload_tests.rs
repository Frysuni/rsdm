use super::*;

fn launch() -> LaunchRequest {
    LaunchRequest {
        generation: "0123456789abcdef0123456789abcdef".into(),
        argv: vec!["example".into()],
        environment: Vec::new(), directory: "/".into(), timeout_secs: 30,
        on_timeout: "force".into(), method: "auto".into(), quit_command: Vec::new(),
    }
}

#[test]
fn count_boundaries_accept_limits_and_reject_the_next_entry() {
    let mut request = launch();
    request.argv = vec!["x".into(); MAX_ARGUMENTS];
    request.environment = vec![("NAME".into(), "x".into()); MAX_ENVIRONMENT];
    request.quit_command = vec!["x".into(); MAX_QUIT_ARGUMENTS];
    validate_launch(&request).unwrap();
    request.argv.push("x".into());
    assert!(validate_launch(&request).unwrap_err().contains("argument limit"));
    request.argv.pop();
    request.environment.push(("NAME".into(), "x".into()));
    assert!(validate_launch(&request).unwrap_err().contains("environment limit"));
    request.environment.pop();
    request.quit_command.push("x".into());
    assert!(validate_launch(&request).unwrap_err().contains("argument limit"));
}

#[test]
fn string_limits_count_utf8_bytes() {
    let mut request = launch();
    request.argv.push("é".repeat(MAX_STRING_BYTES / 2));
    validate_launch(&request).unwrap();
    request.argv[1].push('é');
    assert!(validate_launch(&request).unwrap_err().contains("string exceeds"));
}

#[test]
fn body_limits_include_encoding_and_container_overhead() {
    let mut request = launch();
    request.environment = (0..4)
        .map(|index| (format!("ENV_{index}"), "x".repeat(MAX_STRING_BYTES)))
        .collect();
    assert!(validate_launch(&request).unwrap_err().contains("encoded request exceeds"));
    assert!(validate_finalize(&request.generation, &request.environment).is_err());
    request.environment.pop();
    validate_launch(&request).unwrap();
    validate_finalize(&request.generation, &request.environment).unwrap();
}

#[test]
fn quit_command_has_its_own_encoded_size_limit() {
    let mut request = launch();
    request.quit_command = vec!["x".repeat(MAX_QUIT_BYTES - 9)];
    // Array length (4), string length (4), and NUL (1) are part of the budget.
    validate_launch(&request).unwrap();
    request.quit_command[0].push('x');
    assert!(validate_launch(&request).unwrap_err().contains("encoded request exceeds 65536"));
}

#[test]
fn nul_in_any_launch_string_is_rejected() {
    let request = launch();
    let mut variants = Vec::new();
    let mut argv = request.clone();
    argv.argv[0].push('\0');
    variants.push(argv);
    let mut environment = request.clone();
    environment.environment.push(("NAME".into(), "bad\0value".into()));
    variants.push(environment);
    let mut directory = request.clone();
    directory.directory.push('\0');
    variants.push(directory);
    let mut method = request.clone();
    method.method.push('\0');
    variants.push(method);
    let mut policy = request.clone();
    policy.on_timeout.push('\0');
    variants.push(policy);
    let mut quit = request;
    quit.quit_command.push("bad\0command".into());
    variants.push(quit);
    for request in variants {
        assert!(validate_launch(&request).unwrap_err().contains("NUL"));
    }
}

#[test]
fn other_methods_reject_invalid_or_excessive_inputs() {
    let request = launch();
    assert!(validate_generation(&"x".repeat(MAX_STRING_BYTES + 1)).is_err());
    assert!(validate_stop(&request.generation, "invalid").is_err());
    assert!(validate_finalize(&request.generation, &[("BAD-NAME".into(), "value".into())]).is_err());
    assert!(validate_finalize(&request.generation, &[("NAME".into(), "bad\0value".into())]).is_err());
    let units = vec!["example.service".into(); MAX_ARGUMENTS + 1];
    assert!(validate_xsmp(&request.generation, &units).is_err());
    validate_stop(&request.generation, "logout").unwrap();
    validate_xsmp(&request.generation, &["example.service".into()]).unwrap();
}
