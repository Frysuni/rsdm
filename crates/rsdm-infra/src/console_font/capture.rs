//! Read-only Linux VT ioctls. Font reads must happen while the VT is in text mode.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

use super::ConsoleFont;

#[repr(C)]
struct FontOp {
    op: u32,
    flags: u32,
    width: u32,
    height: u32,
    count: u32,
    data: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UniPair {
    unicode: u16,
    glyph: u16,
}

#[repr(C)]
struct UniMap {
    count: u16,
    entries: *mut UniPair,
}

pub(super) fn read(tty: &str) -> io::Result<ConsoleFont> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY)
        .open(tty)?;
    let fd = file.as_raw_fd();
    // GET_TALL supports fonts taller than 32 pixels; older kernels only have GET.
    let (width, height, bitmap) = read_bitmap(fd, 5, 128).or_else(|_| read_bitmap(fd, 1, 32))?;
    let font = ConsoleFont {
        width,
        height,
        bitmap,
        unicode: read_unicode(fd)?,
    };
    font.validate()?;
    Ok(font)
}

fn read_bitmap(fd: libc::c_int, op: u32, pitch: u32) -> io::Result<(u32, u32, Vec<u8>)> {
    let mut data = vec![0; 8 * pitch as usize * 512];
    let mut request = FontOp {
        op,
        flags: 0,
        width: 64,
        height: pitch,
        count: 512,
        data: data.as_mut_ptr(),
    };
    // SAFETY: KDFONTOP GET writes at most width/8 * pitch * count bytes to data.
    if unsafe { libc::ioctl(fd, 0x4b72, &mut request) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if request.width == 0
        || request.width > 64
        || request.height == 0
        || request.height > pitch
        || request.count == 0
        || request.count > 512
    {
        return Err(super::invalid_font());
    }
    let stride = request.width.div_ceil(8) as usize;
    let bitmap = data
        .chunks_exact(stride * pitch as usize)
        .take(request.count as usize)
        .flat_map(|glyph| glyph[..stride * request.height as usize].iter().copied())
        .collect();
    Ok((request.width, request.height, bitmap))
}

fn read_unicode(fd: libc::c_int) -> io::Result<BTreeMap<char, usize>> {
    let mut entries = vec![UniPair::default(); u16::MAX as usize];
    let mut request = UniMap {
        count: u16::MAX,
        entries: entries.as_mut_ptr(),
    };
    // SAFETY: GIO_UNIMAP writes at most count initialized UniPair entries.
    if unsafe { libc::ioctl(fd, 0x4b66, &mut request) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(entries[..request.count as usize]
        .iter()
        .filter_map(|pair| {
            char::from_u32(u32::from(pair.unicode)).map(|ch| (ch, pair.glyph as usize))
        })
        .collect())
}
