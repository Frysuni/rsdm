use super::*;

#[test]
fn preserves_wide_glyphs_and_unicode_aliases() {
    let font = ConsoleFont {
        width: 12,
        height: 22,
        bitmap: vec![0xa5; 88],
        unicode: [('A', 0), ('─', 1), ('━', 1)].into(),
    };
    let restored = ConsoleFont::from_snapshot(&encode(&font)).unwrap();
    assert_eq!((restored.width(), restored.height()), (12, 22));
    assert_eq!(restored.glyph('A'), Some(&[0xa5; 44][..]));
    assert_eq!(restored.glyph('─'), restored.glyph('━'));
}

#[test]
fn rejects_truncated_and_invalid_snapshots() {
    let font = ConsoleFont {
        width: 8,
        height: 16,
        bitmap: vec![0; 16],
        unicode: [('A', 0)].into(),
    };
    let bytes = encode(&font);
    for length in 0..bytes.len() {
        assert!(ConsoleFont::from_snapshot(&bytes[..length]).is_err());
    }
    let mut unmapped = bytes[..40].to_vec();
    unmapped[20..24].copy_from_slice(&0_u32.to_le_bytes());
    assert!(ConsoleFont::from_snapshot(&unmapped).is_err());
    for (offset, value) in [
        (8, 0_u32),
        (12, 129),
        (16, u32::MAX),
        (20, u32::MAX),
        (44, 1),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(ConsoleFont::from_snapshot(&invalid).is_err());
    }
}

static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rsdm-font-publish-test-{}-{}", std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn snapshot(&self) -> PathBuf {
        self.0.join("console-font-tty1.bin")
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn font(byte: u8) -> ConsoleFont {
    ConsoleFont {
        width: 8,
        height: 16,
        bitmap: vec![byte; 16],
        unicode: [('A', 0)].into(),
    }
}

#[test]
fn stale_pid_temporary_cannot_block_publication() {
    let directory = Directory::new();
    let path = directory.snapshot();
    let stale = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&stale, "stale interrupted write").unwrap();
    write_snapshot(&path, &font(0xa5)).unwrap();

    let restored = ConsoleFont::from_snapshot(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(restored.glyph('A'), Some(&[0xa5; 16][..]));
    assert_eq!(fs::read_to_string(stale).unwrap(), "stale interrupted write");
}

#[test]
fn concurrent_temporaries_use_distinct_exclusively_created_files() {
    let directory = Directory::new();
    let path = directory.snapshot();
    let (first_path, mut first) = create_snapshot_temp(&path).unwrap();
    let (second_path, mut second) = create_snapshot_temp(&path).unwrap();
    assert_ne!(first_path, second_path);
    first.write_all(b"first").unwrap();
    second.write_all(b"second").unwrap();
    assert_eq!(fs::read(first_path).unwrap(), b"first");
    assert_eq!(fs::read(second_path).unwrap(), b"second");
}

#[test]
fn replacement_is_readable_and_leaves_no_temporaries() {
    let directory = Directory::new();
    let path = directory.snapshot();
    write_snapshot(&path, &font(0)).unwrap();
    write_snapshot(&path, &font(0xff)).unwrap();
    let restored = ConsoleFont::from_snapshot(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(restored.glyph('A'), Some(&[0xff; 16][..]));
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[test]
fn failed_rename_removes_only_its_temporary() {
    let directory = Directory::new();
    let path = directory.snapshot();
    fs::create_dir(&path).unwrap();
    let unrelated = directory.0.join("unrelated.tmp");
    fs::write(&unrelated, "keep").unwrap();
    assert!(write_snapshot(&path, &font(0)).is_err());
    assert!(path.is_dir());
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep");
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
}
