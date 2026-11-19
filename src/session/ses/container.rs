//! SD/Quantum `.ses` container: header, section table, bounds-checked reads.
//!
//! Layout notes: `Documentation/SESSION_FILES_FIELD_NOTES.md` §2.1–2.2.

use std::collections::HashMap;

use crate::session::SessionError;
use crate::session::cp1252;

/// Bounds-checked little-endian reader over the whole file.
#[derive(Clone, Copy)]
pub struct Bytes<'a>(pub &'a [u8]);

impl<'a> Bytes<'a> {
    fn need(&self, at: usize, len: usize) -> Result<&'a [u8], SessionError> {
        at.checked_add(len)
            .and_then(|end| self.0.get(at..end))
            .ok_or_else(|| SessionError::format(format!("unexpected end of file at {at:#x}")))
    }

    pub fn u8(&self, at: usize) -> Result<u8, SessionError> {
        Ok(self.need(at, 1)?[0])
    }

    pub fn u16(&self, at: usize) -> Result<u16, SessionError> {
        let b = self.need(at, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, at: usize) -> Result<u32, SessionError> {
        let b = self.need(at, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&self, at: usize) -> Result<f32, SessionError> {
        Ok(f32::from_bits(self.u32(at)?))
    }

    pub fn slice(&self, at: usize, len: usize) -> Result<&'a [u8], SessionError> {
        self.need(at, len)
    }

    /// `u16` length + CP1252 text; returns the text and the offset after it.
    pub fn string16(&self, at: usize) -> Result<(String, usize), SessionError> {
        let len = self.u16(at)? as usize;
        let text = cp1252::decode(self.slice(at + 2, len)?);
        Ok((text, at + 2 + len))
    }

    /// `u8` length + CP1252 text inside a fixed-size field.
    pub fn short_name(&self, at: usize) -> Result<String, SessionError> {
        let len = self.u8(at)? as usize;
        Ok(cp1252::decode(self.slice(at + 1, len)?))
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    /// `SD8`, `SD9`, `SDC` (SD12 family) or `Q2`.
    pub model: String,
    pub version: char,
    pub build: u16,
    /// v9 / vA files use the older offsets and strip-type numbering.
    pub new_layout: bool,
    pub sample_rate: Option<u32>,
}

pub struct Container<'a> {
    pub bytes: Bytes<'a>,
    pub header: Header,
    pub show_name: String,
    /// Section tag → (start, end).
    sections: HashMap<u8, (usize, usize)>,
}

const MAGIC: &[u8] = b"DiGiCo ";

impl<'a> Container<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, SessionError> {
        let bytes = Bytes(data);
        if !data.starts_with(MAGIC) {
            return Err(SessionError::format("not a DiGiCo session file"));
        }
        let model = cp1252::decode(bytes.slice(11, 4)?).trim().to_string();
        let version = bytes.u8(0x15)? as char;
        let first = bytes.u16(0x20)? as usize;
        let new_layout = first != 0x148;
        let build = bytes.u16(0x22)?;
        let rate_at = if new_layout { 0x77 } else { 0x5f };
        let rate = bytes.f32(rate_at)?;
        let sample_rate = (rate == 48000.0 || rate == 96000.0).then_some(rate as u32);

        // Section offset table: u32s ending with the file size.
        let mut at = if new_layout { 0x100 } else { 0x80 };
        let mut offsets = Vec::new();
        loop {
            let off = bytes.u32(at)? as usize;
            if off > data.len() || offsets.len() > 64 {
                return Err(SessionError::format("bad section table"));
            }
            offsets.push(off);
            at += 4;
            if off == data.len() {
                break;
            }
        }
        // Section tag list: the non-zero byte run just before the first section.
        let mut k = first
            .checked_sub(1)
            .ok_or_else(|| SessionError::format("bad header"))?;
        while k > 0 && bytes.u8(k)? == 0 {
            k -= 1;
        }
        let end = k + 1;
        while k > 0 && bytes.u8(k)? != 0 {
            k -= 1;
        }
        let tags = bytes.slice(k + 1, end - (k + 1))?;
        let mut sections = HashMap::new();
        for (i, &tag) in tags.iter().enumerate() {
            if let (Some(&s), Some(&e)) = (offsets.get(i + 1), offsets.get(i + 2)) {
                sections.insert(tag, (s, e));
            }
        }
        // Section 0: u16 (always 0) + u16 length + show name.
        let show_name = match offsets.first() {
            Some(&s) => bytes.string16(s + 2)?.0,
            None => String::new(),
        };
        Ok(Self {
            bytes,
            header: Header {
                model,
                version,
                build,
                new_layout,
                sample_rate,
            },
            show_name,
            sections,
        })
    }

    pub fn section(&self, tag: u8) -> Result<(usize, usize), SessionError> {
        self.sections
            .get(&tag)
            .copied()
            .ok_or_else(|| SessionError::format(format!("section {tag:#04x} missing")))
    }

    /// Snapshot groups and the extra snapshot string exist from format vJ on.
    pub fn has_snapshot_groups(&self) -> bool {
        self.header.version >= 'J'
    }
}
