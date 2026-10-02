//! A tile file's bytes as text, and back.
//!
//! OMSI's own editor saves a tile as UTF-16 little endian with a byte order mark; tiles made
//! by hand are ASCII or Latin-1. Whichever it was, it is written back the same way: a save
//! must not turn a modder's file into something else.

/// How a tile file is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    Latin1,
    Utf16Le,
}

impl Encoding {
    /// The name to show a person.
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Latin1 => "Latin-1",
            Encoding::Utf16Le => "UTF-16LE",
        }
    }
}

/// The text of a tile file and the encoding to write it back in.
pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return (String::from_utf16_lossy(&units), Encoding::Utf16Le);
    }
    match String::from_utf8(bytes.to_vec()) {
        Ok(t) => (t, Encoding::Utf8),
        Err(_) => (bytes.iter().map(|&b| b as char).collect(), Encoding::Latin1),
    }
}

/// Those bytes back, as the tile they came from had them.
pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Latin1 => text.chars().map(|c| c as u32 as u8).collect(),
        Encoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(|u| u.to_le_bytes())).collect(),
    }
}

/// A number as a tile file writes it.
pub fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// One line without its ending, and the ending on its own: an edit must keep the file's own
/// line endings, whatever they are.
pub(crate) fn body(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

pub(crate) fn ending(line: &str) -> &str {
    &line[body(line).len()..]
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: &str = "[version]\r\n4\r\n\r\n[object]\r\n0\r\nSceneryobjects\\a.sco\r\n7\r\n5\r\n6\r\n0.25\r\n90\r\n";

    #[test]
    fn a_utf16_tile_is_written_back_as_utf16() {
        let bytes = encode(TILE, Encoding::Utf16Le);
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'[', 0]);
        let (text, enc) = decode(&bytes);
        assert_eq!((text.as_str(), enc), (TILE, Encoding::Utf16Le));
    }

    #[test]
    fn latin1_is_kept_and_is_not_utf8() {
        // 0xE4 ("ä" in Latin-1) is not valid UTF-8 on its own
        let (text, enc) = decode(b"[object]\r\n0\r\nStra\xe4e.sco\r\n");
        assert_eq!(enc, Encoding::Latin1);
        assert!(text.contains("Stra\u{e4}e"));
        assert_eq!(encode(&text, enc), b"[object]\r\n0\r\nStra\xe4e.sco\r\n".to_vec());
    }

    #[test]
    fn a_number_is_written_the_way_omsi_writes_it() {
        assert_eq!(num(6.5), "6.5");
        assert_eq!(num(5.0), "5");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(102.5), "102.5");
    }

    #[test]
    fn a_line_keeps_its_own_ending() {
        assert_eq!(ending("0\r\n"), "\r\n");
        assert_eq!(ending("0\n"), "\n");
        assert_eq!(ending("0"), "");
        assert_eq!(body("0\r\n"), "0");
    }
}
