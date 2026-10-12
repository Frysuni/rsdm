use super::*;

#[test]
fn only_the_original_process_and_its_live_descendants_belong_to_the_lineage() {
    let pid = std::process::id();
    let handle = ProcessHandle::open(pid).unwrap().unwrap();
    let Some(pidfd_id) = handle.id().unwrap() else { return; };
    let origin = OriginalProcess { pid, pidfd_id };
    assert!(descendant(pid, &handle, &origin).unwrap());
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let child_handle = ProcessHandle::open(child.id()).unwrap().unwrap();
    assert!(descendant(child.id(), &child_handle, &origin).unwrap());
    let foreign = OriginalProcess { pid: child.id(), pidfd_id: child_handle.id().unwrap().unwrap() };
    assert!(!descendant(pid, &handle, &foreign).unwrap());
    assert!(!descendant(pid, &child_handle, &origin).unwrap());
    let stale = OriginalProcess { pid, pidfd_id: pidfd_id.wrapping_add(1) };
    assert!(!descendant(pid, &handle, &stale).unwrap());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!descendant(child.id(), &child_handle, &origin).unwrap());
}

#[test]
fn missing_original_membership_and_non_dedicated_groups_cannot_be_adopted() {
    let pid = std::process::id();
    let handle = ProcessHandle::open(pid).unwrap().unwrap();
    let origin = OriginalProcess { pid, pidfd_id: handle.id().unwrap().unwrap_or(0) };
    for group in ["", "/", "relative"] { assert!(pin_owned(&[pid], group, &origin).unwrap().is_none()); }
    assert!(pin_owned(&[], "/owned.scope", &origin).unwrap().is_none());
    assert!(pin_owned(&[pid], "/nonexistent-rsdm-test.scope", &origin).unwrap().is_none());
}
