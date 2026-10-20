use super::*;

#[test]
fn files_override_unit_values_in_glob_order_and_optional_errors_are_ignored() {
    let directory = std::env::temp_dir().join(format!("rsdm-getty-environment-{}", zbus::Guid::generate()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("one.env"), "NAME=first\nLIST=one 'two three'\n").unwrap();
    std::fs::write(directory.join("two.env"), "NAME=last\n").unwrap();
    std::fs::write(directory.join("invalid"), b"NAME=\xff").unwrap();
    let pattern = directory.join("*.env").display().to_string();
    let missing = directory.join("missing").display().to_string();
    let invalid = directory.join("invalid").display().to_string();
    let base = BTreeMap::from([("NAME".into(), "unit".into())]);
    let values = load_files(base, &[(missing.clone(), true), (pattern, false), (invalid, true)]).unwrap();
    assert_eq!(values["NAME"], "last");
    assert_eq!(values["LIST"], "one 'two three'");
    assert!(load_files(BTreeMap::new(), &[(missing, false)]).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unit_assignments_are_validated_without_expanding_their_values() {
    let values = assignments(vec!["NAME=first".into(), "NAME=$VALUE".into(), "1BAD=ignored".into()]);
    assert_eq!(values, BTreeMap::from([("NAME".into(), "$VALUE".into())]));
}
