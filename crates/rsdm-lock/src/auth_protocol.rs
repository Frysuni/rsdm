use std::io::{self, Read, Write};

use rsdm_core::ports::{AuthMessage, AuthMessageStyle, MAX_PASSWORD_BYTES, MAX_USERNAME_BYTES};
use rsdm_core::domain::PasswordSecret;

pub(crate) const MAX_SERVICE_BYTES: usize = 256;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) enum Frame {
    Start {
        username: String,
        service: String,
        password: String,
    },
    Message(AuthMessage),
    Answer(Option<String>),
    Cancel,
    Finished { success: bool, message: String },
}

pub(crate) fn encode_start(username: &str, service: &str, password: &PasswordSecret) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let frame = Frame::Start {
        username: username.to_string(),
        service: service.to_string(),
        password: password.expose_secret().to_string(),
    };
    let payload = encode_payload(&frame)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "authentication frame too large"));
    }
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

pub(crate) fn write_frame(stream: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let payload = encode_payload(frame)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "authentication frame too large"));
    }
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(&payload)
}

pub(crate) fn read_frame(stream: &mut impl Read) -> io::Result<Frame> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "authentication frame too large"));
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    decode_payload(&payload)
}

pub(crate) fn take_frames(buffer: &mut Vec<u8>) -> io::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    loop {
        if buffer.len() < 4 {
            break;
        }
        let length = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
        if length > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "authentication frame too large"));
        }
        if buffer.len() < 4 + length {
            break;
        }
        let payload = buffer[4..4 + length].to_vec();
        buffer.drain(..4 + length);
        frames.push(decode_payload(&payload)?);
    }
    Ok(frames)
}

fn encode_payload(frame: &Frame) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    match frame {
        Frame::Start { username, service, password } => {
            validate_text(username, MAX_USERNAME_BYTES, "username")?;
            validate_text(service, MAX_SERVICE_BYTES, "PAM service")?;
            validate_text(password, MAX_PASSWORD_BYTES, "password")?;
            payload.push(1);
            put_text(&mut payload, username)?;
            put_text(&mut payload, service)?;
            put_text(&mut payload, password)?;
        }
        Frame::Message(message) => {
            validate_text(&message.text, MAX_TEXT_BYTES, "PAM message")?;
            payload.push(2);
            payload.push(style_byte(message.style));
            put_text(&mut payload, &message.text)?;
        }
        Frame::Answer(answer) => {
            payload.push(3);
            match answer {
                Some(answer) => {
                    validate_text(answer, MAX_PASSWORD_BYTES, "PAM answer")?;
                    payload.push(1);
                    put_text(&mut payload, answer)?;
                }
                None => payload.push(0),
            }
        }
        Frame::Cancel => payload.push(4),
        Frame::Finished { success, message } => {
            validate_text(message, MAX_TEXT_BYTES, "authentication result")?;
            payload.push(5);
            payload.push(u8::from(*success));
            put_text(&mut payload, message)?;
        }
    }
    Ok(payload)
}

fn decode_payload(payload: &[u8]) -> io::Result<Frame> {
    let (&tag, mut rest) = payload
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty authentication frame"))?;
    match tag {
        1 => Ok(Frame::Start {
            username: take_text(&mut rest, MAX_USERNAME_BYTES, "username")?,
            service: take_text(&mut rest, MAX_SERVICE_BYTES, "PAM service")?,
            password: take_text(&mut rest, MAX_PASSWORD_BYTES, "password")?,
        }),
        2 => {
            let (&style, tail) = rest
                .split_first()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM message style"))?;
            rest = tail;
            let style = parse_style(style)?;
            Ok(Frame::Message(AuthMessage { style, text: take_text(&mut rest, MAX_TEXT_BYTES, "PAM message")? }))
        }
        3 => {
            let (&present, tail) = rest
                .split_first()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM answer flag"))?;
            rest = tail;
            let answer = match present {
                0 => None,
                1 => Some(take_text(&mut rest, MAX_PASSWORD_BYTES, "PAM answer")?),
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid PAM answer flag")),
            };
            ensure_empty(rest)?;
            Ok(Frame::Answer(answer))
        }
        4 => {
            ensure_empty(rest)?;
            Ok(Frame::Cancel)
        }
        5 => {
            let (&success, tail) = rest
                .split_first()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing authentication result"))?;
            rest = tail;
            if success > 1 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid authentication result"));
            }
            Ok(Frame::Finished { success: success == 1, message: take_text(&mut rest, MAX_TEXT_BYTES, "authentication result")? })
        }
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown authentication frame")),
    }
}

fn put_text(payload: &mut Vec<u8>, text: &str) -> io::Result<()> {
    let bytes = text.as_bytes();
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "authentication text too large"))?;
    payload.extend_from_slice(&length.to_be_bytes());
    payload.extend_from_slice(bytes);
    Ok(())
}

fn take_text(rest: &mut &[u8], maximum: usize, name: &str) -> io::Result<String> {
    if rest.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("missing {name} length")));
    }
    let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
    *rest = &rest[4..];
    if length > maximum || rest.len() < length {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("invalid {name} length")));
    }
    let text = std::str::from_utf8(&rest[..length])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("invalid {name}")))?
        .to_string();
    *rest = &rest[length..];
    Ok(text)
}

fn validate_text(text: &str, maximum: usize, name: &str) -> io::Result<()> {
    if text.len() > maximum || text.as_bytes().contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid {name}")));
    }
    Ok(())
}

fn ensure_empty(rest: &[u8]) -> io::Result<()> {
    if rest.is_empty() { Ok(()) } else { Err(io::Error::new(io::ErrorKind::InvalidData, "trailing authentication frame data")) }
}

fn style_byte(style: AuthMessageStyle) -> u8 {
    match style {
        AuthMessageStyle::Secret => 1,
        AuthMessageStyle::Visible => 2,
        AuthMessageStyle::Info => 3,
        AuthMessageStyle::Error => 4,
    }
}

fn parse_style(style: u8) -> io::Result<AuthMessageStyle> {
    match style {
        1 => Ok(AuthMessageStyle::Secret),
        2 => Ok(AuthMessageStyle::Visible),
        3 => Ok(AuthMessageStyle::Info),
        4 => Ok(AuthMessageStyle::Error),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid PAM message style")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_frame_round_trips_with_bounded_secret() {
        let secret = PasswordSecret::new("correct horse battery staple");
        let encoded = encode_start("alice", "login", &secret).unwrap();
        let mut stream = &encoded[..];
        assert!(matches!(read_frame(&mut stream), Ok(Frame::Start { username, service, password }) if username == "alice" && service == "login" && password == "correct horse battery staple"));
    }

    #[test]
    fn malformed_frame_is_rejected_before_allocation() {
        let mut bytes = (u32::MAX).to_be_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        assert!(read_frame(&mut &bytes[..]).is_err());
    }

    #[test]
    fn partial_frames_wait_for_more_bytes() {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &Frame::Cancel).unwrap();
        let split = encoded.len() - 1;
        let mut buffer = encoded[..split].to_vec();
        assert!(take_frames(&mut buffer).unwrap().is_empty());
        buffer.push(encoded[split]);
        assert!(matches!(take_frames(&mut buffer).unwrap().as_slice(), [Frame::Cancel]));
    }
}
