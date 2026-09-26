//! The Greeter's Linux console font, shared with the unprivileged Lock.

mod capture;
mod snapshot;

use std::{collections::BTreeMap, io};

pub use snapshot::{load, publish};

/// MSB-first bitmap rows and the console's Unicode-to-glyph mapping.
pub struct ConsoleFont {
    width: u32,
    height: u32,
    bitmap: Vec<u8>,
    unicode: BTreeMap<char, usize>,
}

impl ConsoleFont {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn glyph(&self, ch: char) -> Option<&[u8]> {
        let index = self
            .unicode
            .get(&ch)
            .or_else(|| self.unicode.get(&'�'))
            .or_else(|| self.unicode.get(&'?'))?;
        let size = (self.width.div_ceil(8) * self.height) as usize;
        self.bitmap.get(index * size..(index + 1) * size)
    }

    fn validate(&self) -> io::Result<()> {
        if !(1..=64).contains(&self.width) || !(1..=128).contains(&self.height) {
            return Err(invalid_font());
        }
        let size = (self.width.div_ceil(8) * self.height) as usize;
        let count = self.bitmap.len() / size;
        if count == 0
            || count > 512
            || !self.bitmap.len().is_multiple_of(size)
            || self.unicode.values().any(|&index| index >= count)
        {
            return Err(invalid_font());
        }
        Ok(())
    }
}

fn invalid_font() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid console font snapshot")
}
