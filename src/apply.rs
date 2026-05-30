use crate::error::RtpError;

// Opcode values from S9 of the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    End        = 0x01,
    SetSource  = 0x02,
    Copy       = 0x03,
    CopyGap    = 0x04,
    Flush      = 0x05,
    Poke1      = 0x06,
    Poke1xN    = 0x07,
    Store      = 0x08,
    Tcopy      = 0x09,
    TcopyGap   = 0x0A,
    Zfill      = 0x0B,
    ZfillGap   = 0x0C,
    Poke1xNVar = 0x0D,
    Poke1xNB   = 0x0E,
    Poke16xN   = 0x0F,
    Poke32xN   = 0x10,
    Fill1      = 0x11,
    Fill2      = 0x12,
    Fill4      = 0x13,
    Fill1Gap   = 0x14,
    Fill2Gap   = 0x15,
    Fill4Gap   = 0x16,
}

impl TryFrom<u8> for Opcode {
    type Error = u8;

    fn try_from(b: u8) -> Result<Self, u8> {
        match b {
            0x01 => Ok(Self::End),
            0x02 => Ok(Self::SetSource),
            0x03 => Ok(Self::Copy),
            0x04 => Ok(Self::CopyGap),
            0x05 => Ok(Self::Flush),
            0x06 => Ok(Self::Poke1),
            0x07 => Ok(Self::Poke1xN),
            0x08 => Ok(Self::Store),
            0x09 => Ok(Self::Tcopy),
            0x0A => Ok(Self::TcopyGap),
            0x0B => Ok(Self::Zfill),
            0x0C => Ok(Self::ZfillGap),
            0x0D => Ok(Self::Poke1xNVar),
            0x0E => Ok(Self::Poke1xNB),
            0x0F => Ok(Self::Poke16xN),
            0x10 => Ok(Self::Poke32xN),
            0x11 => Ok(Self::Fill1),
            0x12 => Ok(Self::Fill2),
            0x13 => Ok(Self::Fill4),
            0x14 => Ok(Self::Fill1Gap),
            0x15 => Ok(Self::Fill2Gap),
            0x16 => Ok(Self::Fill4Gap),
            other => Err(other),
        }
    }
}

// Opcode stream VLI reader (same encoding as file-level VLI, S5 of spec).
struct OpcodeReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> OpcodeReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn byte(&mut self) -> Result<u8, &'static str> {
        if self.pos >= self.data.len() {
            return Err("opcode stream truncated");
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if self.pos + n > self.data.len() {
            return Err("opcode stream truncated");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn vli(&mut self) -> Result<i64, &'static str> {
        let b = self.byte()? as u64;
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
                tail |= (self.byte()? as u64) << (8 * i);
            }
            (high << (8 * count)) | tail
        };
        Ok(if sign { -(val as i64) } else { val as i64 })
    }

    fn next_opcode(&mut self) -> Option<Result<Opcode, String>> {
        let b = self.byte().ok()?;
        Some(Opcode::try_from(b).map_err(|b| {
            format!("unknown opcode 0x{b:02x} at stream offset {}", self.pos - 1)
        }))
    }
}

/// Apply a MODIFY patch opcode stream to `src_data`, returning the patched bytes.
pub fn apply(
    src_data: &[u8],
    opcodes: &[u8],
    dest_size: usize,
    src_count: i64,
) -> Result<Vec<u8>, RtpError> {
    run_opcodes(src_data, opcodes, dest_size, src_count)
        .map_err(|e| RtpError::ApplyError { filename: String::new(), detail: e })
}

fn run_opcodes(
    src: &[u8],
    opcodes: &[u8],
    dest_size: usize,
    src_count: i64,
) -> Result<Vec<u8>, String> {
    let mut r = OpcodeReader::new(opcodes);
    let mut output = vec![0u8; dest_size];
    let mut out_pos: usize = 0;
    let mut poke_pos: i64 = 0;
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut templates: Vec<(usize, usize)> = Vec::new();

    fn grow(output: &mut Vec<u8>, n: usize) {
        if n > output.len() {
            output.resize(n, 0);
        }
    }

    fn do_copy(src: &[u8], output: &mut Vec<u8>, out_pos: &mut usize,
               gaps: &mut Vec<(usize, usize)>, advance: usize, src_off: usize, cnt: usize) {
        if advance > 0 {
            gaps.push((*out_pos, advance));
            *out_pos += advance;
        }
        grow(output, *out_pos + cnt);
        let src_end = (src_off + cnt).min(src.len());
        let copy_len = src_end.saturating_sub(src_off);
        if copy_len > 0 {
            output[*out_pos..*out_pos + copy_len].copy_from_slice(&src[src_off..src_end]);
        }
        *out_pos += cnt;
    }

    fn do_fill(output: &mut Vec<u8>, out_pos: &mut usize, gaps: &mut Vec<(usize, usize)>,
               seek: usize, pattern: &[u8], count: usize) {
        if seek > 0 {
            gaps.push((*out_pos, seek));
            *out_pos += seek;
        }
        grow(output, *out_pos + count);
        if !pattern.is_empty() && pattern.iter().any(|&b| b != 0) {
            let plen = pattern.len();
            for i in 0..count {
                output[*out_pos + i] = pattern[i % plen];
            }
        }
        *out_pos += count;
    }

    fn poke_delta(output: &mut Vec<u8>, pos: i64, width: usize, delta: i64) {
        if pos < 0 { return; }
        let p = pos as usize;
        grow(output, p + width);
        let cur: i64 = match width {
            1 => output[p] as i64,
            2 => i16::from_le_bytes(output[p..p+2].try_into().unwrap()) as i64,
            4 => i32::from_le_bytes(output[p..p+4].try_into().unwrap()) as i64,
            _ => return,
        };
        let v = cur.wrapping_add(delta);
        match width {
            1 => output[p] = v as u8,
            2 => output[p..p+2].copy_from_slice(&(v as i16).to_le_bytes()),
            4 => output[p..p+4].copy_from_slice(&(v as i32).to_le_bytes()),
            _ => {}
        }
    }

    fn s8(b: u8) -> i64 { if b >= 128 { b as i64 - 256 } else { b as i64 } }

    while let Some(op_result) = r.next_opcode() {
        let op = op_result?;
        match op {
            Opcode::End => break,

            Opcode::SetSource => {
                let _src_idx = r.vli().map_err(|e| e.to_string())?;
                out_pos = 0;
                poke_pos = 0;
            }

            Opcode::Copy | Opcode::CopyGap => {
                let advance = if op == Opcode::CopyGap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let _src_idx = if src_count >= 2 { r.vli().map_err(|e| e.to_string())? } else { 0 };
                let src_off = r.vli().map_err(|e| e.to_string())? as usize;
                let cnt = r.vli().map_err(|e| e.to_string())? as usize;
                do_copy(src, &mut output, &mut out_pos, &mut gaps, advance, src_off, cnt);
            }

            Opcode::Flush => {
                if dest_size > out_pos {
                    gaps.push((out_pos, dest_size - out_pos));
                }
                for &(off, ln) in &gaps {
                    grow(&mut output, off + ln);
                    let bytes = r.bytes(ln).map_err(|e| e.to_string())?;
                    output[off..off + ln].copy_from_slice(bytes);
                }
                gaps.clear();
            }

            Opcode::Poke1 => {
                let seek = r.vli().map_err(|e| e.to_string())?;
                let delta = s8(r.byte().map_err(|e| e.to_string())?);
                poke_pos += seek;
                poke_delta(&mut output, poke_pos, 1, delta);
            }

            Opcode::Poke1xN | Opcode::Poke1xNB => {
                poke_pos = 0;
                let delta = s8(r.byte().map_err(|e| e.to_string())?);
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                for _ in 0..count {
                    poke_pos += r.vli().map_err(|e| e.to_string())?;
                    poke_delta(&mut output, poke_pos, 1, delta);
                }
            }

            Opcode::Store => {
                let _src_idx = if src_count >= 2 { r.vli().map_err(|e| e.to_string())? } else { 0 };
                let src_off = r.vli().map_err(|e| e.to_string())? as usize;
                let cnt = r.vli().map_err(|e| e.to_string())? as usize;
                templates.push((src_off, cnt));
            }

            Opcode::Tcopy | Opcode::TcopyGap => {
                let advance = if op == Opcode::TcopyGap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let idx = r.vli().map_err(|e| e.to_string())? as usize;
                let &(src_off, cnt) = templates.get(idx)
                    .ok_or_else(|| format!("TCOPY: template index {idx} out of range"))?;
                do_copy(src, &mut output, &mut out_pos, &mut gaps, advance, src_off, cnt);
            }

            Opcode::Zfill | Opcode::ZfillGap => {
                let seek = if op == Opcode::ZfillGap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                do_fill(&mut output, &mut out_pos, &mut gaps, seek, &[], count);
            }

            Opcode::Poke1xNVar => {
                poke_pos = 0;
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                for _ in 0..count {
                    poke_pos += r.vli().map_err(|e| e.to_string())?;
                    let delta = s8(r.byte().map_err(|e| e.to_string())?);
                    poke_delta(&mut output, poke_pos, 1, delta);
                }
            }

            Opcode::Poke16xN => {
                poke_pos = 0;
                let delta = i16::from_le_bytes(
                    r.bytes(2).map_err(|e| e.to_string())?.try_into().unwrap()
                ) as i64;
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                for _ in 0..count {
                    poke_pos += r.vli().map_err(|e| e.to_string())?;
                    poke_delta(&mut output, poke_pos, 2, delta);
                }
            }

            Opcode::Poke32xN => {
                poke_pos = 0;
                let delta = i32::from_le_bytes(
                    r.bytes(4).map_err(|e| e.to_string())?.try_into().unwrap()
                ) as i64;
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                for _ in 0..count {
                    poke_pos += r.vli().map_err(|e| e.to_string())?;
                    poke_delta(&mut output, poke_pos, 4, delta);
                }
            }

            Opcode::Fill1 | Opcode::Fill1Gap => {
                let seek = if op == Opcode::Fill1Gap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let pat = r.bytes(1).map_err(|e| e.to_string())?.to_vec();
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                do_fill(&mut output, &mut out_pos, &mut gaps, seek, &pat, count);
            }

            Opcode::Fill2 | Opcode::Fill2Gap => {
                let seek = if op == Opcode::Fill2Gap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let pat = r.bytes(2).map_err(|e| e.to_string())?.to_vec();
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                do_fill(&mut output, &mut out_pos, &mut gaps, seek, &pat, count);
            }

            Opcode::Fill4 | Opcode::Fill4Gap => {
                let seek = if op == Opcode::Fill4Gap {
                    r.vli().map_err(|e| e.to_string())? as usize
                } else { 0 };
                let pat = r.bytes(4).map_err(|e| e.to_string())?.to_vec();
                let count = r.vli().map_err(|e| e.to_string())? as usize;
                do_fill(&mut output, &mut out_pos, &mut gaps, seek, &pat, count);
            }
        }
    }

    output.truncate(dest_size);
    Ok(output)
}
