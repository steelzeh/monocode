//! Language servers exchange JSON messages, each one after a `Content-Length`
//! header.
use serde_json::Value;
use std::io::{self, BufRead, Write};

/// No answer from a server is this large.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// Reads one message. `None` means that the other side closed its output.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length
        .filter(|length| *length <= MAX_MESSAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad message length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(Into::into)
}

pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn messages_survive_a_round_trip_with_multi_byte_text() {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &json!({"text": "kø 🙂"})).unwrap();
        write_message(&mut bytes, &json!({"id": 2})).unwrap();
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({"text": "kø 🙂"}))
        );
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"id": 2})));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }
    #[test]
    fn other_headers_are_skipped_and_case_does_not_matter() {
        let text = "Content-Type: x\r\ncontent-LENGTH: 2\r\n\r\n{}";
        assert_eq!(
            read_message(&mut Cursor::new(text)).unwrap(),
            Some(json!({}))
        );
    }
    #[test]
    fn a_message_without_a_length_is_an_error() {
        assert!(read_message(&mut Cursor::new("X: 1\r\n\r\n{}")).is_err());
    }
}
