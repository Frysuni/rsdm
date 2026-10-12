use super::*;

#[test]
fn a_changed_cgroup_inode_cannot_restore_a_previous_observation() {
    let path = process_group(std::process::id()).unwrap();
    if path == "/" { return; }
    let group = Group::open(&path).unwrap();
    assert!(Group::restore(&group.identity).unwrap().is_some());
    let mut replaced = group.identity.clone();
    replaced.inode = replaced.inode.wrapping_add(1);
    assert!(Group::restore(&replaced).unwrap().is_none());
}

#[test]
fn pinned_observation_reads_the_original_file_after_name_reuse() {
    let directory = std::env::temp_dir().join(format!("rsdm-cgroup-observation-{}", zbus::Guid::generate()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("cgroup.events");
    fs::write(&path, "populated 1\n").unwrap();
    let file = File::open(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, "populated 0\n").unwrap();
    assert_eq!(read(&file).unwrap(), "populated 1\n");
    fs::remove_dir_all(directory).unwrap();
}
