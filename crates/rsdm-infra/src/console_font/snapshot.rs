//! Bounded, atomic font snapshots in the DM's runtime directory.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{ConsoleFont, capture, invalid_font};

const MAGIC: &[u8; 8] = b"RSDMFNT1";
const MAX_BYTES: u64 = 1024 * 1024;

pub fn publish(tty: &str) -> io::Result<()> {
    let path = snapshot_path(tty)?;
    let font = capture::read(tty)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temp)?;
    let result = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.write_all(&encode(&font))?;
        fs::rename(&temp, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub fn load(tty: &str) -> io::Result<ConsoleFont> {
    let mut data = Vec::new();
    File::open(snapshot_path(tty)?)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut data)?;
    ConsoleFont::from_snapshot(&data)
}

fn snapshot_path(tty: &str) -> io::Result<PathBuf> {
    let name = Path::new(tty)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid_font)?;
    if !name.starts_with("tty")
        || name.len() == 3
        || !name[3..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_font());
    }
    Ok(Path::new("/run/rsdm").join(format!("console-font-{name}.bin")))
}

fn encode(font: &ConsoleFont) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for value in [
        font.width,
        font.height,
        font.bitmap.len() as u32,
        font.unicode.len() as u32,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&font.bitmap);
    for (&ch, &index) in &font.unicode {
        bytes.extend_from_slice(&(ch as u32).to_le_bytes());
        bytes.extend_from_slice(&(index as u32).to_le_bytes());
    }
    bytes
}

impl ConsoleFont {
    /// Decode a bounded console font snapshot produced by DM.
    pub fn from_snapshot(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() < 24 || bytes.len() as u64 > MAX_BYTES || &bytes[..8] != MAGIC {
            return Err(invalid_font());
        }
        let number = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let bitmap_len = number(16) as usize;
        let entries = number(20) as usize;
        if bitmap_len > MAX_BYTES as usize
            || entries > u16::MAX as usize
            || bytes.len() != 24 + bitmap_len + entries * 8
        {
            return Err(invalid_font());
        }
        let mut unicode = BTreeMap::new();
        for offset in (24 + bitmap_len..bytes.len()).step_by(8) {
            let ch = char::from_u32(number(offset)).ok_or_else(invalid_font)?;
            unicode.insert(ch, number(offset + 4) as usize);
        }
        let font = ConsoleFont {
            width: number(8),
            height: number(12),
            bitmap: bytes[24..24 + bitmap_len].to_vec(),
            unicode,
        };
        font.validate()?;
        Ok(font)
    }
}

#[cfg(test)]
mod tests {
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
}
