//! Fresh identifiers shared by the DM launcher and Session Manager.

use std::{fs::File, io::Read};

pub fn new_generation() -> Result<String, std::io::Error> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
