/// Minimal SCALE codec for the Polkadot app host-api wire format.
///
/// Implements only the primitives used by the protocol:
/// compact integers, strings, bytes, enums (u8 tag), options, results, vectors.

// ---------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------

pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    fn need(&self, n: usize) -> Result<(), DecodeErr> {
        if self.pos + n > self.data.len() {
            Err(DecodeErr::Eof)
        } else {
            Ok(())
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeErr> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_u32_le(&mut self) -> Result<u32, DecodeErr> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn read_raw(&mut self, n: usize) -> Result<&'a [u8], DecodeErr> {
        self.need(n)?;
        let v = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(v)
    }

    /// SCALE compact integer (unsigned, up to u32 range).
    pub fn read_compact_u32(&mut self) -> Result<u32, DecodeErr> {
        let first = self.read_u8()? as u32;
        match first & 0b11 {
            0b00 => Ok(first >> 2),
            0b01 => {
                let second = self.read_u8()? as u32;
                Ok(((first | (second << 8)) >> 2) & 0x3FFF)
            }
            0b10 => {
                self.need(3)?;
                let b1 = self.data[self.pos] as u32;
                let b2 = self.data[self.pos + 1] as u32;
                let b3 = self.data[self.pos + 2] as u32;
                self.pos += 3;
                let val = first | (b1 << 8) | (b2 << 16) | (b3 << 24);
                Ok(val >> 2)
            }
            0b11 => {
                let byte_count = (first >> 2) + 4;
                if byte_count > 4 {
                    return Err(DecodeErr::CompactTooLarge);
                }
                let mut val = 0u32;
                for i in 0..byte_count as usize {
                    val |= (self.read_u8()? as u32) << (i * 8);
                }
                Ok(val)
            }
            _ => unreachable!(),
        }
    }

    /// SCALE string: compact length + UTF-8 bytes.
    pub fn read_string(&mut self) -> Result<String, DecodeErr> {
        let len = self.read_compact_u32()? as usize;
        let bytes = self.read_raw(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeErr::InvalidUtf8)
    }

    /// Dynamic-length bytes: compact length + raw bytes.
    pub fn read_var_bytes(&mut self) -> Result<Vec<u8>, DecodeErr> {
        let len = self.read_compact_u32()? as usize;
        Ok(self.read_raw(len)?.to_vec())
    }

    /// Fixed-length bytes.
    pub fn read_fixed_bytes(&mut self, n: usize) -> Result<Vec<u8>, DecodeErr> {
        Ok(self.read_raw(n)?.to_vec())
    }

    /// SCALE Option: 0x00 = None, 0x01 = Some(T).
    pub fn read_option<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, DecodeErr>,
    ) -> Result<Option<T>, DecodeErr> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            _ => Err(DecodeErr::InvalidOption),
        }
    }

    /// Skip all remaining bytes (for void / don't-care payloads).
    pub fn skip_rest(&mut self) {
        self.pos = self.data.len();
    }
}

#[derive(Debug)]
pub enum DecodeErr {
    Eof,
    CompactTooLarge,
    InvalidUtf8,
    InvalidOption,
    InvalidTag(u8),
    BadMessage(&'static str),
}

impl std::fmt::Display for DecodeErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => write!(f, "unexpected end of input"),
            Self::CompactTooLarge => write!(f, "compact integer exceeds u32"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in string"),
            Self::InvalidOption => write!(f, "invalid option discriminant"),
            Self::InvalidTag(t) => write!(f, "invalid tag: {t}"),
            Self::BadMessage(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DecodeErr {}

// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------

/// SCALE compact integer encode (u32 range).
pub fn encode_compact_u32(buf: &mut Vec<u8>, val: u32) {
    if val < 0x40 {
        buf.push((val as u8) << 2);
    } else if val < 0x4000 {
        let v = (val << 2) | 0b01;
        buf.push(v as u8);
        buf.push((v >> 8) as u8);
    } else if val < 0x4000_0000 {
        let v = (val << 2) | 0b10;
        buf.push(v as u8);
        buf.push((v >> 8) as u8);
        buf.push((v >> 16) as u8);
        buf.push((v >> 24) as u8);
    } else {
        buf.push(0b11); // mode 3, 0 extra bytes indicator = 4 bytes total
        buf.push(val as u8);
        buf.push((val >> 8) as u8);
        buf.push((val >> 16) as u8);
        buf.push((val >> 24) as u8);
    }
}

/// SCALE string: compact length + UTF-8 bytes.
pub fn encode_string(buf: &mut Vec<u8>, s: &str) {
    encode_compact_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

/// Enum tag (u8).
pub fn encode_tag(buf: &mut Vec<u8>, tag: u8) {
    buf.push(tag);
}

/// Result::Ok(void) = [0x00].
pub fn encode_result_ok_void(buf: &mut Vec<u8>) {
    buf.push(0x00);
}

/// Result::Ok with inner value.
pub fn encode_result_ok(buf: &mut Vec<u8>) {
    buf.push(0x00);
}

/// Result::Err with inner error.
pub fn encode_result_err(buf: &mut Vec<u8>) {
    buf.push(0x01);
}

/// Dynamic-length bytes: compact length + raw bytes.
pub fn encode_var_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    encode_compact_u32(buf, data.len() as u32);
    buf.extend_from_slice(data);
}

/// Option::None = [0x00].
pub fn encode_option_none(buf: &mut Vec<u8>) {
    buf.push(0x00);
}

/// Option::Some prefix = [0x01], then caller writes the value.
pub fn encode_option_some(buf: &mut Vec<u8>) {
    buf.push(0x01);
}

/// Vector: compact count + items (caller encodes each item).
pub fn encode_vector_len(buf: &mut Vec<u8>, count: u32) {
    encode_compact_u32(buf, count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_round_trip() {
        for val in [0u32, 1, 63, 64, 16383, 16384, 1_073_741_823, u32::MAX] {
            let mut buf = Vec::new();
            encode_compact_u32(&mut buf, val);
            let mut r = Reader::new(&buf);
            let decoded = r.read_compact_u32().unwrap();
            assert_eq!(val, decoded, "compact round-trip failed for {val}");
            assert_eq!(r.pos, buf.len());
        }
    }

    #[test]
    fn string_round_trip() {
        for s in ["", "hello", "dot://mytestapp.dot", "a".repeat(1000).as_str()] {
            let mut buf = Vec::new();
            encode_string(&mut buf, s);
            let mut r = Reader::new(&buf);
            let decoded = r.read_string().unwrap();
            assert_eq!(s, decoded);
        }
    }

    #[test]
    fn var_bytes_round_trip() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let mut buf = Vec::new();
        encode_var_bytes(&mut buf, &data);
        let mut r = Reader::new(&buf);
        let decoded = r.read_var_bytes().unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn var_bytes_empty() {
        let mut buf = Vec::new();
        encode_var_bytes(&mut buf, &[]);
        assert_eq!(buf, vec![0x00]); // compact(0)
        let mut r = Reader::new(&buf);
        let decoded = r.read_var_bytes().unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn reader_eof_on_empty() {
        let mut r = Reader::new(&[]);
        assert!(r.read_u8().is_err());
        assert!(r.read_string().is_err());
        assert!(r.read_compact_u32().is_err());
    }

    #[test]
    fn reader_truncated_string() {
        // Compact length says 10 bytes but only 3 available
        let mut buf = Vec::new();
        encode_compact_u32(&mut buf, 10);
        buf.extend_from_slice(b"abc");
        let mut r = Reader::new(&buf);
        assert!(r.read_string().is_err());
    }
}
