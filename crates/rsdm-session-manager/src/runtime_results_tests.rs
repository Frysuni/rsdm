use super::*;

#[test]
fn querying_absent_results_does_not_create_an_archive() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    assert!(runtime.archived_apps().unwrap().is_empty());
    assert!(!runtime.path.join("results").exists());
}

#[test]
fn archived_records_remain_visible_after_application_retirement() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let record = app();
    runtime.save_app(&record).unwrap();
    runtime.archive_app(&record).unwrap();
    runtime.remove_app(&record.unit).unwrap();
    assert!(runtime.apps().unwrap().is_empty());
    let archived = runtime.archived_apps().unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].unit, record.unit);
    assert_eq!(archived[0].invocation_id, record.invocation_id);
}

#[test]
fn archive_queries_reject_symlinks_and_foreign_generations() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let record = app();
    runtime.archive_app(&record).unwrap();
    let archive = runtime.path.join("results");
    fs::rename(&archive, runtime.path.join("outside")).unwrap();
    symlink(runtime.path.join("outside"), &archive).unwrap();
    assert!(runtime.archived_apps().is_err());
    fs::remove_file(&archive).unwrap();
    fs::rename(runtime.path.join("outside"), &archive).unwrap();
    fs::rename(archive.join(format!("{}.toml", record.unit)), archive.join("app-rsdm-example@other-1.service.toml")).unwrap();
    assert!(runtime.archived_apps().is_err());
}
