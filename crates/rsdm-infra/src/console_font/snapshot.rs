//! Bounded, atomic font snapshots in the DM's runtime directory.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::{io::Write, os::unix::fs::PermissionsExt};

use super::{ConsoleFont, capture, invalid_font};

const MAGIC: &[u8; 8] = b"RSDMFNT1";
const MAX_BYTES: u64 = 1024 * 1024;

pub fn publish(tty: &str) -> io::Result<()> {
    let path = snapshot_path(tty)?;
    let font = capture::read(tty)?;
    write_snapshot(&path, &font)
}

fn write_snapshot(path: &Path, font: &ConsoleFont) -> io::Result<()> {
    let parent = path.parent().ok_or_else(invalid_font)?;
    fs::create_dir_all(parent)?;
    crate::atomic_file::write(
        path,
        &encode(font),
        crate::atomic_file::AtomicWriteOptions {
            mode: 0o644,
            sync_file: true,
            sync_parent: true,
        },
    )
}

#[cfg(test)]
fn create_snapshot_temp(path: &Path) -> io::Result<(std::path::PathBuf, File)> {
    crate::atomic_file::create_temp(path, 0o644)
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
#[path = "snapshot_tests.rs"]
mod tests;
