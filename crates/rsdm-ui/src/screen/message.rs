//! Multiline authentication messages shared by Greeter and Lock.

use crate::text::{Role, Segment};

use super::BodyLine;

pub(super) fn push_message(body: &mut Vec<BodyLine>, message: &str, role: Role, width: u16) {
    body.push(BodyLine::Blank);
    let width = usize::from(width.max(1));
    for line in message.lines() {
        let mut row = String::new();
        for (index, ch) in line.chars().enumerate() {
            if index > 0 && index % width == 0 {
                body.push(BodyLine::Centered(vec![Segment::bold(row, role)]));
                row = String::new();
            }
            row.push(if ch.is_control() { ' ' } else { ch });
        }
        body.push(BodyLine::Centered(vec![Segment::bold(row, role)]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_lines_wraps_text_and_sanitizes_control_characters() {
        let mut body = Vec::new();
        push_message(&mut body, "123456\nOTP:\t", Role::Info, 4);
        let rows: Vec<&str> = body.iter().filter_map(|line| match line {
            BodyLine::Centered(segments) => Some(segments[0].text.as_str()),
            _ => None,
        }).collect();
        assert_eq!(rows, ["1234", "56", "OTP:", " "]);
    }
}
