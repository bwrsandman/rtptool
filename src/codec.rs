// Faithful port of rtp_codec.py (patchw32.dll FUN_1001b780 et al.)
// Level/group/slot adaptive Huffman + LZSS.

const DIFF_MAGIC: u32 = 0xB59C;
const WINDOW_FLAG_8KB: u32 = 8;

// MSB-first bit reader. pos points to the byte currently being consumed;
// bl = bits still available in buf[pos] (1..8).
struct BitIn {
    pos: usize,
    end: usize,
    bl: u32,
}

impl BitIn {
    fn new(offset: usize, end: usize) -> Self {
        Self { pos: offset, end, bl: 8 }
    }

    fn bit(&mut self, buf: &[u8]) -> Result<u8, &'static str> {
        if self.pos >= self.end {
            return Err("bitstream truncated");
        }
        let b = buf[self.pos];
        let v = (b >> (self.bl - 1)) & 1;
        self.bl -= 1;
        if self.bl == 0 {
            self.pos += 1;
            self.bl = 8;
        }
        Ok(v)
    }

    fn bits(&mut self, n: u32, buf: &[u8]) -> Result<u32, &'static str> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit(buf)? as u32;
        }
        Ok(v)
    }
}

// Adaptive Huffman tree: flat byte buffer with field offsets matching the DLL.
struct HuffTree {
    m: Vec<u8>,
}

impl HuffTree {
    fn r16u(&self, off: usize) -> u32 {
        self.m[off] as u32 | ((self.m[off + 1] as u32) << 8)
    }

    fn r16s(&self, off: usize) -> i32 {
        let v = self.r16u(off);
        if v & 0x8000 != 0 { v as i32 - 0x10000 } else { v as i32 }
    }

    fn w16(&mut self, off: usize, val: i32) {
        let v = (val & 0xFFFF) as u16;
        self.m[off]     = v as u8;
        self.m[off + 1] = (v >> 8) as u8;
    }

    fn r32(&self, off: usize) -> usize {
        self.m[off] as usize
            | ((self.m[off + 1] as usize) << 8)
            | ((self.m[off + 2] as usize) << 16)
            | ((self.m[off + 3] as usize) << 24)
    }

    fn w32(&mut self, off: usize, val: usize) {
        self.m[off]     =  val        as u8;
        self.m[off + 1] = (val >>  8) as u8;
        self.m[off + 2] = (val >> 16) as u8;
        self.m[off + 3] = (val >> 24) as u8;
    }

    fn new(esc_bits: u32, num_levels: usize, init_period: u32, upd_period: u32) -> Self {
        let alpha = 1usize << esc_bits;
        let off_groupcnt = 0x34usize;
        let off_symtab   = num_levels * 2 + 0x34;
        let off_slot     = num_levels * 6 + 0x38;
        let off_weight   = num_levels * 6 + 0x40 + alpha * 4;
        let off_limit    = off_weight + 4 + alpha * 2;
        let size         = off_limit + 0xC0 + 0x100;

        let mut t = HuffTree { m: vec![0u8; size] };

        t.w16(0x32, init_period as i32);
        t.w16(0x02, init_period as i32);
        t.w16(0x00, init_period as i32);
        t.w16(0x30, upd_period  as i32);
        t.w16(0x2e, upd_period  as i32);
        t.w16(0x2c, upd_period  as i32);
        t.w16(0x06, esc_bits    as i32);
        t.w16(0x04, num_levels  as i32);
        t.w32(0x0c, off_groupcnt);
        t.w32(0x20, off_symtab);
        t.w32(0x1c, off_slot);
        t.w32(0x18, off_weight);
        t.w32(0x14, off_limit);
        t.w32(0x10, off_limit);

        t.w16(0x0a, 1);
        t.w16(0x08, 1);
        t.w16(off_groupcnt, 2);
        for i in 1..num_levels {
            t.w16(off_groupcnt + i * 2, 0);
        }

        t.w32(off_symtab, off_slot);
        for i in 1..=(num_levels) {
            t.w32(off_symtab + i * 4, off_slot + 8);
        }

        let wbase = off_weight + alpha * 2;
        t.w32(off_slot, wbase);
        for i in 1..=(alpha + 1) {
            t.w32(off_slot + i * 4, wbase + 2);
        }

        for j in 0..alpha {
            t.w16(off_weight + j * 2, 0x8000u16 as i32);
        }
        t.w16(off_weight + alpha * 2,     0);
        t.w16(off_weight + alpha * 2 + 2, 0);

        t.w16(0x24, alpha as i32);
        let lim = t.r32(0x10);
        for k in 0..0x30 {
            t.w32(lim + k * 4, 0);
        }

        t.build_limits(0);
        t
    }

    fn build_limits(&mut self, start: usize) {
        let gc  = self.r32(0x0c);
        let lim = self.r32(0x10);
        let num = self.r16u(0x04) as usize;

        let mut s3: i32 = if start == 0 {
            2
        } else {
            self.r16s(lim + (start - 1) * 8) * 2
        };

        for level in start..num {
            let s4 = s3 - self.r16s(gc + level * 2);
            self.w16(lim + level * 8,     s4);
            s3 = s4 * 2;
            self.w16(lim + level * 8 + 2, s3);
            self.w16(lim + level * 8 + 4, s4 * 4);
            self.w16(lim + level * 8 + 6, s4 * 16);
        }
    }

    // Returns true when the countdown reaches zero (rebuild needed).
    fn update_freq(&mut self, sym: usize) -> bool {
        let w   = self.r32(0x18);
        let cur = self.r16u(w + sym * 2);
        self.w16(w + sym * 2, (cur + 1) as i32);
        let cnt = self.r16s(0x00) - 1;
        self.w16(0x00, cnt);
        (cnt & 0xFFFF) == 0
    }

    // Register a new symbol; returns the lowest affected level for build_limits.
    fn add_symbol(&mut self, newsym: usize) -> usize {
        let w    = self.r32(0x18);
        let slot = self.r32(0x1c);
        let gc   = self.r32(0x0c);
        let st   = self.r32(0x20);
        let iv   = newsym * 2;
        self.w16(w + iv, 1);
        let n_slots = self.r16u(0x08) as usize;
        self.w32(slot + n_slots * 4, w + iv);
        let n_slots = n_slots + 1;
        self.w16(0x08, n_slots as i32);
        if n_slots != 2 {
            let n_groups = self.r16u(0x0a) as usize;
            let num      = self.r16u(0x04) as usize;
            let uvar6 = if n_groups < num {
                let u = n_groups.wrapping_sub(1) & 0xFFFF;
                self.w16(0x0a, (n_groups + 1) as i32);
                u
            } else {
                let mut u = n_groups.wrapping_sub(2) & 0xFFFF;
                while self.r16s(gc + u * 2) == 0 {
                    u = u.wrapping_sub(1) & 0xFFFF;
                }
                u
            };
            let v = self.r16s(gc + uvar6 * 2);          self.w16(gc + uvar6 * 2, v - 1);
            let v = self.r16s(gc + 2 + uvar6 * 2);      self.w16(gc + 2 + uvar6 * 2, v + 2);
            let v = self.r32(st + (uvar6 + 1) * 4);     self.w32(st + (uvar6 + 1) * 4, v.wrapping_sub(4));
            for u in (uvar6 + 2)..=(num) {
                let v = self.r32(st + u * 4);
                self.w32(st + u * 4, v.wrapping_add(4));
            }
            return uvar6;
        }
        0
    }

    fn rebuild(&mut self) {
        let slot    = self.r32(0x1c);
        let symt    = self.r32(0x20);
        let gc      = self.r32(0x0c);
        let n_slots = self.r16u(0x08) as usize;
        let upd     = self.r16s(0x2c);
        let num     = self.r16u(0x04) as usize;
        let mut maxw: u32 = 0;

        self.w16(0x2c, upd - 1);

        // Block A: optionally halve weights, find max.
        if n_slots != 0 {
            for i in 0..n_slots {
                let p = self.r32(slot + i * 4);
                let mut wv = self.r16u(p);
                if ((upd - 1) & 0xFFFF) == 0 {
                    wv >>= 1;
                    self.w16(p, wv as i32);
                }
                if maxw < wv { maxw = wv; }
            }
        }

        // Block B: bit-radix partition by weight descending.
        if maxw != 0 {
            let mut mask = 0x8000u32;
            while (maxw & mask) == 0 {
                mask = (mask >> 1) | 0x8000;
            }
            let mut la: usize = 0;
            if n_slots != 0 {
                'outer: while la < n_slots {
                    let pcur_addr = slot + la * 4;
                    let pcur = self.r32(pcur_addr);
                    if (self.r16u(pcur) & mask) == 0 {
                        let mut u17 = la + 1;
                        let mut ins = la;
                        if n_slots <= u17 { break; }
                        let mut u5 = ins;
                        while u17 < n_slots {
                            let pp = slot + u17 * 4;
                            let p9 = self.r32(pp);
                            u5 = ins;
                            if (self.r16u(p9) & mask) != 0 {
                                u5 = ins + 1;
                                let pcur_val = self.r32(slot + ins * 4);
                                self.w32(slot + ins * 4, p9);
                                self.w32(pp, pcur_val);
                                let _ = self.r32(slot + u5 * 4); // pcur = slot[u5]
                            }
                            u17 += 1;
                            ins = u5;
                        }
                        if u5 != la { la = u5 - 1; }
                        let nm = mask >> 1;
                        mask = nm | 0x8000;
                        if (nm & 1) != 0 { break 'outer; }
                    } else {
                        la += 1;
                    }
                }
            }
        }

        // Block C: group rebalance.
        let mut la: usize = 0;
        let mut n_groups = self.r16u(0x0a) as usize;
        let mut ustk = n_groups.wrapping_sub(1) & 0xFFFF;
        let mut moved: i32 = 0;

        if n_groups != 0 {
            loop {
                let pgc = gc + la * 2;
                let p2  = symt + la * 4;
                let g   = self.r16u(pgc) as usize;
                if g == 0 {
                    la += 1;
                } else {
                    let p1      = self.r32(p2 + 4);
                    let w_first = self.r16u(self.r32(self.r32(p2)));
                    let w_last  = self.r16u(self.r32(p1.wrapping_sub(4)));
                    let w_last2 = self.r16u(self.r32(p1.wrapping_sub(8)));
                    if (g < 3)
                        || (((num.wrapping_sub(1)) & 0xFFFF) == la)
                        || (w_first < w_last + w_last2)
                    {
                        // merge branch
                        let mut p16   = p2 + 4;
                        let mut found = false;
                        let acc0      = self.r16u(self.r32(p1.wrapping_sub(4)));
                        let mut acc   = acc0 as i32;
                        let mut u17   = la + 2;
                        while u17 < n_groups {
                            let p4 = symt + u17 * 4;
                            let w  = self.r16s(self.r32(self.r32(p4)));
                            acc = (acc - w) & 0xFFFF;
                            let gcnt17 = self.r16u(gc + u17 * 2);
                            let wn     = self.r16u(self.r32(self.r32(p4).wrapping_add(4)));
                            if gcnt17 > 1
                                && ((acc & 0x8000) != 0 || (acc as u32) < wn)
                            {
                                found = true;
                                break;
                            }
                            u17 += 1;
                        }
                        if !found {
                            la += 1;
                        } else {
                            let v = self.r16s(pgc);         self.w16(pgc, v - 1);
                            moved += 1;
                            la += 1;
                            let v = self.r32(p16);          self.w32(p16, v.wrapping_sub(4));
                            let v = self.r16s(gc + la * 2); self.w16(gc + la * 2, v + 2);
                            let v = self.r32(p2 + 8);       self.w32(p2 + 8, v.wrapping_add(4));
                            if la < u17.wrapping_sub(1) & 0xFFFF {
                                let span = u17.wrapping_sub(1) - la;
                                la += span;
                                for _ in 0..span {
                                    let v = self.r32(p16 + 8); self.w32(p16 + 8, v.wrapping_add(4));
                                    p16 += 4;
                                }
                            }
                            let v = self.r16s(gc + la * 2);     self.w16(gc + la * 2, v + 1);
                            let v = self.r32(p16 + 4);          self.w32(p16 + 4, v.wrapping_add(4));
                            let v = self.r16s(gc + 2 + la * 2); self.w16(gc + 2 + la * 2, v - 2);
                            if self.r16s(gc + ustk * 2) == 0 {
                                n_groups -= 1;
                                self.w16(0x0a, n_groups as i32);
                                ustk = ustk.wrapping_sub(1) & 0xFFFF;
                            }
                            la = 0;
                        }
                    } else {
                        // simple branch
                        moved += 1;
                        let v = self.r16s(pgc.wrapping_sub(2)); self.w16(pgc.wrapping_sub(2), v + 1);
                        let v = self.r16s(pgc);                 self.w16(pgc, v - 3);
                        let v = self.r16s(gc + 2 + la * 2);     self.w16(gc + 2 + la * 2, v + 2);
                        let v = self.r32(p2);                   self.w32(p2, v.wrapping_add(4));
                        let v = self.r32(p2 + 4);               self.w32(p2 + 4, v.wrapping_sub(8));
                        if ustk == la {
                            n_groups += 1;
                            self.w16(0x0a, n_groups as i32);
                            ustk = ustk.wrapping_add(1) & 0xFFFF;
                        }
                        la = 0;
                    }
                }
                if la >= n_groups { break; }
            }
        }

        // Tail: adjust periods.
        if moved < 0x10 {
            if moved < 8 && self.r16u(0x2e) != 1 {
                let v = self.r16u(0x02); self.w16(0x02, (v << 1) as i32);
                let v = self.r16u(0x2c); self.w16(0x2c, (v >> 1) as i32);
                let v = self.r16u(0x2e); self.w16(0x2e, (v >> 1) as i32);
            }
        } else {
            let v = self.r16u(0x32); self.w16(0x02, v as i32);
            let v = self.r16u(0x30); self.w16(0x2e, v as i32);
        }
        let v = self.r16u(0x02); self.w16(0x00, v as i32);
        if self.r16u(0x2c) == 0 {
            let v = self.r16u(0x2e); self.w16(0x2c, v as i32);
        }
    }

    fn decode(&mut self, bi: &mut BitIn, buf: &[u8]) -> Result<u16, String> {
        if bi.pos >= bi.end {
            return Err("bitstream truncated".into());
        }
        let bl   = bi.bl as i32;
        let cur  = buf[bi.pos] as i32;
        let mut val = ((1i32 << bl) - 1) & cur;
        let mut idx = bl - 1;
        let mut tot = bl;
        let lim = self.r32(0x10);

        if (val as u32) < self.r16u(lim + (idx as usize) * 8) {
            loop {
                bi.pos += 1;
                if bi.pos >= bi.end {
                    return Err("bitstream truncated".into());
                }
                idx += 8;
                tot += 8;
                let nb = buf[bi.pos] as i32;
                val = (((val & 0xFF) << 8) | nb) & 0xFFFF;
                if (val as u32) >= self.r16u(lim + (idx as usize) * 8) {
                    break;
                }
            }
        }

        idx -= 1;
        let mut cnt  = (tot - 1) & 0xFF;
        let mut local3: i32 = 0;
        if cnt != 0 {
            loop {
                let lim_off = lim.wrapping_add(2).wrapping_add((idx as usize).wrapping_mul(8));
                let threshold = if lim_off < self.m.len().saturating_sub(1) {
                    self.r16u(lim_off) as i32
                } else {
                    0
                };
                if val < threshold {
                    break;
                }
                idx   -= 1;
                cnt   -= 1;
                val  >>= 1;
                local3 += 1;
                if cnt == 0 { break; }
            }
        }

        let symt  = self.r32(0x20);
        let level = ((idx + 1) & 0xFFFF) as usize;
        let slot_arr = self.r32(symt + level * 4);
        let k = (val - self.r16s(lim + level * 8)) as usize & 0xFFFF;
        let p = self.r32(slot_arr + k * 4);
        let off_weight = self.r32(0x18);
        let sym = ((p.wrapping_sub(off_weight)) >> 1) as u16;

        if local3 == 0 {
            local3 = 8;
            bi.pos += 1;
        }
        bi.bl = local3 as u32;

        if self.update_freq(sym as usize) {
            self.rebuild();
            self.build_limits(0);
        }

        let esc = self.r16u(0x24) as u16;
        if sym == esc {
            let raw = bi.bits(self.r16u(0x06), buf)
                .map_err(|e| e.to_string())?;
            let lvl = self.add_symbol(raw as usize);
            self.build_limits(lvl);
            return Ok(raw as u16);
        }

        Ok(sym)
    }
}

/// Decompress a MODIFY record's compressed diff payload.
pub fn decompress(
    data: &[u8],
    offset: usize,
    size: usize,
    max_output: usize,
) -> Result<Vec<u8>, String> {
    let end = if size > 0 { (offset + size).min(data.len()) } else { data.len() };
    if offset >= data.len() {
        return Err(format!("offset 0x{offset:x} beyond data length {}", data.len()));
    }

    let mut bi = BitIn::new(offset, end);

    let magic = bi.bits(16, data).map_err(|e| e.to_string())?;
    if magic != DIFF_MAGIC {
        return Err(format!("bad diff magic: 0x{magic:04x} (want 0x{DIFF_MAGIC:04x})"));
    }
    let use_raw     = bi.bits(8,  data).map_err(|e| e.to_string())?;
    let _reserved   = bi.bits(8,  data).map_err(|e| e.to_string())?;
    let init_period = bi.bits(12, data).map_err(|e| e.to_string())?;
    let upd_period  = bi.bits(12, data).map_err(|e| e.to_string())?;
    let wflag       = bi.bits(4,  data).map_err(|e| e.to_string())?;
    let dist_bits   = if wflag == WINDOW_FLAG_8KB { 7u32 } else { 6 };

    let mut lit_tree  = if use_raw == 0 {
        Some(HuffTree::new(8, 0x10, init_period, upd_period))
    } else {
        None
    };
    let mut len_tree  = HuffTree::new(6, 0x0C, init_period, upd_period);
    let mut dist_tree = HuffTree::new(6, 0x0C, init_period, upd_period);

    let mut out: Vec<u8> = Vec::with_capacity(max_output.min(1 << 20));

    loop {
        if out.len() >= max_output { break; }

        let flag = bi.bit(data).map_err(|e| e.to_string())?;

        if flag == 0 {
            let sym = if let Some(ref mut lt) = lit_tree {
                lt.decode(&mut bi, data).map_err(|e| e.to_string())?
            } else {
                bi.bits(8, data).map_err(|e| e.to_string())? as u16
            };
            out.push(sym as u8);
        } else {
            let dist_lo = bi.bits(dist_bits, data).map_err(|e| e.to_string())?;
            let dist_hi = dist_tree.decode(&mut bi, data).map_err(|e| e.to_string())? as u32;
            let dist = (dist_hi << dist_bits) | dist_lo;
            if dist == 0 { break; }
            let length = (len_tree.decode(&mut bi, data).map_err(|e| e.to_string())? & 0x7F) as usize;
            let back = dist as usize + 1;
            for _ in 0..length {
                if out.len() >= max_output { break; }
                let pos = out.len();
                let byte = if pos >= back { out[pos - back] } else { 0 };
                out.push(byte);
            }
        }
    }

    Ok(out)
}
