//! Windows-1252 text decoding for SD/Quantum `.ses` strings.
//!
//! Snapshot names and notes are CP1252 (accented French notes decode correctly
//! only this way). Bytes 0x80–0x9F map to the table below; every other byte is
//! the Latin-1 code point of the same value. Undefined bytes become U+FFFD.

const HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

/// Decodes CP1252 bytes into a `String`.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[(b - 0x80) as usize],
            _ => char::from(b),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_latin1_pass_through() {
        assert_eq!(decode(b"Ngoma1"), "Ngoma1");
        // "entrée" and "Système" as the SD writes them.
        assert_eq!(decode(b"entr\xe9e"), "entrée");
        assert_eq!(decode(b"Syst\xe8me"), "Système");
    }

    #[test]
    fn the_windows_range_maps_to_its_own_characters() {
        assert_eq!(decode(b"\x80 \x85 \x92 \x96"), "€ … ’ –");
        assert_eq!(decode(b"\x81"), "\u{FFFD}");
    }
}
