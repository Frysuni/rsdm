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

fn frames() -> [Frame; 6] {
    [
        Frame::Start { username: "alice".into(), service: "login".into(), password: "secret".into() },
        Frame::Message(AuthMessage { style: AuthMessageStyle::Secret, text: "Password:".into() }),
        Frame::Answer(Some("secret".into())),
        Frame::Answer(None),
        Frame::Cancel,
        Frame::Finished { success: true, message: "accepted".into() },
    ]
}

#[test]
fn every_frame_rejects_trailing_payload_in_both_readers() {
    for frame in frames() {
        let mut payload = encode_payload(&frame).unwrap();
        payload.push(0);
        let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&payload);
        assert_eq!(read_frame(&mut &bytes[..]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(take_frames(&mut bytes).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn every_text_field_rejects_nul_bytes() {
    for frame in frames() {
        let payload = encode_payload(&frame).unwrap();
        for index in 0..payload.len() {
            if !payload[index].is_ascii_alphabetic() {
                continue;
            }
            let mut corrupted = payload.clone();
            corrupted[index] = 0;
            assert_eq!(decode_payload(&corrupted).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }
}

#[test]
fn adjacent_frames_remain_separate_and_preserve_order() {
    let mut bytes = Vec::new();
    for frame in frames() {
        write_frame(&mut bytes, &frame).unwrap();
    }
    assert!(matches!(take_frames(&mut bytes).unwrap().as_slice(), [
        Frame::Start { .. }, Frame::Message(_), Frame::Answer(Some(_)),
        Frame::Answer(None), Frame::Cancel, Frame::Finished { success: true, .. },
    ]));
    assert!(bytes.is_empty());
}
