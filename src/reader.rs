use crate::error::RtpError;

pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn read_exact(&mut self, n: usize) -> Result<&'a [u8], RtpError> {
        if self.pos + n > self.data.len() {
            return Err(RtpError::UnexpectedEof {
                offset: self.pos,
                needed: n,
                available: self.data.len() - self.pos,
            });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, RtpError> {
        Ok(self.read_exact(1)?[0])
    }

    pub fn u16_le(&mut self) -> Result<u16, RtpError> {
        let b = self.read_exact(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32_le(&mut self) -> Result<u32, RtpError> {
        let b = self.read_exact(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// ANSI NUL-terminated length-prefixed string.
    /// Length byte includes the NUL. 0xFF → u16 length follows.
    pub fn lp_string(&mut self) -> Result<String, RtpError> {
        let len = self.u8()? as usize;
        let len = if len == 0xFF {
            self.u16_le()? as usize
        } else {
            len
        };
        if len == 0 {
            return Ok(String::new());
        }
        let raw = self.read_exact(len)?;
        let text = if raw.last() == Some(&0) { &raw[..raw.len() - 1] } else { raw };
        Ok(String::from_utf8_lossy(text).into_owned())
    }

    /// Variable-length integer per §5.
    /// Lead byte: bit7=sign, consecutive 1-bits from bit6 = count of continuation bytes.
    /// Continuation bytes are little-endian.
    pub fn vli(&mut self) -> Result<i64, RtpError> {
        let b = self.u8()? as u64;
        let sign = (b & 0x80) != 0;

        let mut count = 0u32;
        let mut mask = 0x40u64;
        while mask > 0 && (b & mask) != 0 {
            count += 1;
            mask >>= 1;
        }

        let val = if count == 0 {
            b & 0x3F
        } else {
            let extract_mask = (0x40u64 >> count) - 1;
            let high = b & extract_mask;
            let mut tail = 0u64;
            for i in 0..count {
                tail |= (self.u8()? as u64) << (8 * i);
            }
            (high << (8 * count)) | tail
        };

        Ok(if sign { -(val as i64) } else { val as i64 })
    }
}
