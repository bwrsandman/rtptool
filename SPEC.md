# RTPatch (.rtp) Binary Patch Format

> This format specification was reverse-engineered with the assistance of Claude (Anthropic).
> The binary layout, field semantics, VLI encoding, and adaptive Huffman + LZSS codec were
> all decoded by AI analysis of the binary and its behaviour — no official documentation exists.

A technical description of the RTPatch container and diff format, corresponding
to **RTPatch version 2.09** (PATCH file version 6.01).

An `.rtp` file is a self-contained patch describing how to transform a set of
source files into their updated versions. It holds a header, an optional
directory table, and a sequence of per-file records. MODIFY records embed a
compressed *opcode stream* that reconstructs each destination file from its
source.

Conventions used below:
- Multi-byte fixed-width integers are **little-endian**.
- `u8`, `u16`, `u32` denote unsigned fixed-width integers.
- `VLI` is the variable-length integer of §5.
- `lp_string` is the length-prefixed string of §4.

---

## 1. Container overview

```
+------------------+
| File header      |
+------------------+
| Directory table  |   optional
+------------------+
| File record 0    |
| File record 1    |
| ...              |
| EOF record       |   record type nibble = 1
+------------------+
```

Three encodings coexist:
1. Fixed-width little-endian integers (header, entry descriptors).
2. A byte-oriented **VLI** for counts, seeks and opcode operands (§5).
3. A bit-oriented **MSB-first bit stream** for the compressed diff payload
   (§7–9).

---

## 2. File header

Fields are read in order; some are conditional on preceding flag bits.

| Field            | Type | Presence | Meaning |
|------------------|------|----------|---------|
| magic            | u8×2 | always   | `"K*"` |
| version          | u16  | always   | format version (≤ 2.09) |
| flags            | u16  | always   | container flags (see below) |
| ext_type_flags   | u32  | if `flags` bit 15 set | extended type/option flags |
| option_flags     | u16  | always   | default per-record option flags |
| patch_total_size | u32  | always   | total size of the patch payload region |
| reserved_a       | u32  | always   | reserved |
| default_attrs    | u16  | always   | default file attributes for records |
| reserved_b       | u16  | always   | reserved |
| cmd_flags        | u16  | always   | command flags; bit 2 gates the next field |
| combine_mode_id  | u32  | if `cmd_flags` bit 2 set | combine/merge identifier |
| reserved_c       | u32  | always   | reserved |

Flag semantics:
- `flags` bit 15 — an `ext_type_flags` word is present.
- `flags` high byte, bit 1 — a directory table follows the header (§3).
- `ext_type_flags`, low 3 bits — must be zero in this version.
- `ext_type_flags`, bit 16 — **extra mode**. When set, records and entry
  descriptors carry additional timestamp and alternate-path fields. Affects
  parsing throughout (§4, §6).

---

## 3. Directory table

Present only when the directory-table flag is set.

| Field   | Type | Meaning |
|---------|------|---------|
| n_dirs  | u16  | number of directory strings |
| dirs[]  | n_dirs × lp_string | directory paths |

The table provides directory context for reconstructing full output paths;
records still carry their own short names and optional explicit paths.

---

## 4. Length-prefixed string (lp_string)

ANSI, NUL-terminated, with a length prefix that *includes* the terminator:

| Field  | Type   | Notes |
|--------|--------|-------|
| length | u8     | byte count including the NUL; value `0xFF` escapes to a following u16 |
| (length2) | u16 | present only when `length == 0xFF` |
| data   | length bytes | string bytes ending in a NUL; the textual value is `data` minus its final byte |

An empty string is encoded as `length == 0`.

---

## 5. Variable-length integer (VLI)

A VLI is one lead byte followed by zero or more continuation bytes:

```
lead byte b:
    sign  = b & 0x80                       (bit 7)
    count = number of consecutive 1-bits beginning at bit 6, scanning downward
            (test masks 0x40, 0x20, 0x10, 0x08, 0x04 ...)

if count == 0:
    value = b & 0x3F                       (single byte; range 0..63)
else:
    high  = b & ((0x40 >> count) - 1)      (the lead byte's remaining low bits)
    tail  = the next `count` bytes read as a LITTLE-ENDIAN integer
    value = (high << (8 * count)) | tail

if sign: value = -value
```

The lead byte contributes the most-significant bits; the continuation bytes
are the lower bytes in **little-endian** order.

Worked example — bytes `60 E1 64`:
- `b = 0x60` → sign 0; one 1-bit at bit 6, then a 0-bit → `count = 2`.
- `high = 0x60 & 0x0F = 0`.
- `tail = 0xE1 | (0x64 << 8) = 0x64E1 = 25825`.
- `value = 25825`.

Endianness matters only for values that need continuation bytes (≥ 64). Single
-byte VLIs are interpretation-independent.

The `count` field can in principle reach a 5-byte form (count 4); only the
1–3-byte forms are commonly seen.

---

## 6. File records

Each record begins with a `rec_hdr` u16:
- bits 15–12: **record type** (the nibble).
- bits 11–0: per-record flag bits.

| Type | Name   | Inline payload |
|------|--------|----------------|
| 1    | EOF    | none — terminates the record sequence |
| 2    | RENAME | none |
| 3    | NEW    | none inline; content supplied externally |
| 4    | MODIFY | compressed opcode diff stream |
| 5    | MKDIR  | directory record (distinct fields) |
| 6    | DELETE | none |

### 6.1 Record fields

Read in order, each gated by a flag (record type from the nibble; remaining
bits of `rec_hdr` named by position):

| Condition                                  | Field |
|--------------------------------------------|-------|
| always                                     | `rec_hdr` u16 |
| `rec_hdr` bit 1                            | `option_flags` override (u16) |
| `rec_hdr` bit 2                            | filename lp_string |
| `option_flags` bits 6–7 set                | seek VLI |
| `option_flags` bits 6–7 set **and** extra mode | checksum VLI |
| `rec_hdr` bit 7                            | disk-set VLI |
| `rec_hdr` bit 8                            | attribute override (u16) |
| `rec_hdr` bit 9 and type ≠ 5              | source-path + dest-path lp_strings |
| type == 5                                  | u32 + u16 directory-specific fields |
| always                                     | 10-byte fixed metadata block |
| type ≠ 5                                    | data block (§6.2) |

### 6.2 Data block (non-directory records)

```
if type == 4 (MODIFY):
    file_mod_flags  u16
    src_count       VLI
elif type == 3 (NEW):
    src_count       VLI
    src_count × entry           # destination size = first entry's file size
    (record ends here)
else (type 2, 6):
    src_count = 0

# types 2, 4, 6 continue:
dst_count      VLI
reserved       u32
payload_len    u32              # length of the inline compressed diff
src_count × entry
dst_count × entry               # destination size = first dest entry's file size
<compressed diff: payload_len bytes>     # present for MODIFY
```

### 6.3 Entry descriptor

Each source/destination entry describes one file version:

| Part         | Size | Contents |
|--------------|------|----------|
| descriptor   | 24   | short (8.3) name, attribute flags, and the **file size** (u32) |
| checksum blk | 10   | flags and a **CRC32** (stored masked to its low 30 bits) |
| timestamps   | 8    | present only in extra mode |
| alt-path     | lp_string | present only in extra mode |

The reconstructed output size is the file-size field of the first destination
entry (MODIFY) or first source entry (NEW). The CRC is the source-validation
value (§10).

---

## 7. Compressed diff container

A MODIFY record's payload is a single MSB-first bit stream. Its header bits are
consumed in order, most-significant bit first:

| Bits | Field        | Meaning |
|------|--------------|---------|
| 16   | magic        | `0xB59C` |
| 8    | literal_mode | 0 → literals use adaptive Huffman; nonzero → raw 8-bit literals |
| 8    | reserved     | consumed and discarded |
| 12   | init_period  | Huffman frequency-reset period |
| 12   | upd_period   | Huffman update period |
| 4    | window_flag  | `8` → 8 KB window (7 distance low-bits); otherwise 4 KB (6 low-bits) |

Following the header, three adaptive-Huffman alphabets are initialised by
reading further bits: a **literal** alphabet (256 symbols), a **length**
alphabet (64 symbols) and a **distance** alphabet (64 symbols). All three draw
from the same bit cursor.

---

## 8. Adaptive Huffman + LZSS decompression

The diff is an order-0 **adaptive Huffman** code over an **LZSS** token stream.
Each alphabet maintains symbol frequencies, a level/group structure and a limit
table used for canonical decoding. After each decoded symbol the model updates
that symbol's weight; on a periodic schedule it rebuilds (halving weights and
re-partitioning symbols by weight, with the update period self-tuning to how
much the structure changed). Unseen literals are introduced through an
**escape symbol**: decoding it triggers reading a fixed number of raw bits for
the new literal value, which is then registered into the alphabet.

### Token loop

```
repeat until the end sentinel or the output is complete:
    flag = 1 bit
    if flag == 0:                              # literal token
        sym = literal_mode ? (8 raw bits) : decode(literal_alphabet)
        emit byte sym
    else:                                      # back-reference token
        dist_lo = (window low-bits) raw bits
        dist_hi = decode(distance_alphabet)
        dist    = (dist_hi << window_low_bits) | dist_lo
        if dist == 0:                          # end-of-stream sentinel
            stop
        length  = decode(length_alphabet) & 0x7F
        back    = dist + 1
        repeat length times:
            if (output_len - back) >= 0:
                emit output[output_len - back]
            else:
                emit 0x00                      # window is zero-initialised
```

Back-references that reach before the current output start read **zero**,
matching a zero-initialised sliding window. This is required for
highly-compressible inputs containing long zero runs.

The decompressed result is the **opcode stream** consumed in §9.

---

## 9. Opcode stream (file reconstruction)

The decompressed bytes form a program that builds the destination file. Each
opcode is one byte; operands are VLIs unless noted. The interpreter maintains:

- **write cursor** — the monotonically increasing output position.
- **poke cursor** — a separate running position for in-place delta edits.
- **gap list** — recorded `(offset, length)` literal holes, filled at flush.
- **template list** — stored copy descriptors `{source, offset, count}`.
- The output buffer is the full destination size, **zero-initialised**.

### Opcode table

| Op   | Name        | Operands | Effect |
|------|-------------|----------|--------|
| 0x01 | END         | —        | terminate |
| 0x02 | SET_SOURCE  | src VLI  | select active source; reset write and poke cursors to 0 |
| 0x03 | COPY        | [src VLI if multi-source] off VLI, cnt VLI | copy `cnt` bytes from source at absolute `off` to the write cursor; advance cursor |
| 0x04 | COPY+gap    | adv VLI, [src] off, cnt | record a gap of `adv` at the cursor, skip it, then COPY |
| 0x05 | FLUSH       | —        | record a final gap to end-of-file, then fill **every** recorded gap in order with consecutive literal bytes taken from the stream; clear the list |
| 0x06 | POKE1       | seek VLI, delta s8 | poke cursor += seek; add signed `delta` to the 1 byte there (no reset) |
| 0x07 | POKE1×N     | delta s8, count VLI, count×(seek VLI) | reset poke cursor; N times: cursor += seek, add `delta` to 1 byte |
| 0x08 | STORE       | [src] off VLI, cnt VLI | append `{source, off, cnt}` to the template list (emits nothing) |
| 0x09 | TCOPY       | idx VLI  | COPY using template `idx` |
| 0x0A | TCOPY+gap   | adv VLI, idx VLI | gap of `adv`, then COPY using template `idx` |
| 0x0B | ZFILL       | count VLI | write `count` zero bytes |
| 0x0C | ZFILL+gap   | seek VLI, count VLI | gap, then zero-fill |
| 0x0D | POKE1×N var | count VLI, count×(seek VLI, delta s8) | reset poke cursor; per entry: cursor += seek, add `delta` |
| 0x0E | POKE1×N     | delta s8, count VLI, count×(seek VLI) | constant 1-byte delta at N running positions |
| 0x0F | POKE16×N    | delta s16, count VLI, count×(seek VLI) | constant little-endian 2-byte delta at N positions |
| 0x10 | POKE32×N    | delta s32, count VLI, count×(seek VLI) | constant little-endian 4-byte delta at N positions |
| 0x11 | FILL1       | pattern[1], count VLI | repeat a 1-byte pattern `count` times |
| 0x12 | FILL2       | pattern[2], count VLI | repeat a 2-byte pattern |
| 0x13 | FILL4       | pattern[4], count VLI | repeat a 4-byte pattern |
| 0x14 | FILL1+gap   | seek VLI, pattern[1], count VLI | gap, then 1-byte pattern fill |
| 0x15 | FILL2+gap   | seek VLI, pattern[2], count VLI | gap, then 2-byte pattern fill |
| 0x16 | FILL4+gap   | seek VLI, pattern[4], count VLI | gap, then 4-byte pattern fill |

### Semantics

- **Copies** address the source at an **absolute** offset and write `cnt` bytes
  at the write cursor. A leading *advance* first records a literal gap and
  skips the cursor past it.
- **Templates** (0x08) seed a 0-based, store-ordered dictionary of frequently
  re-used source runs. They produce no output by themselves; 0x09/0x0A copy
  through them. This lets recurring inserted strings be encoded once.
- **Pokes are delta-adds, not overwrites.** The width-byte little-endian value
  at the poke cursor is incremented by a signed delta. The poke cursor
  accumulates across an op's entries and resets to 0 at the start of ops
  0x02/0x07/0x0D/0x0E/0x0F/0x10 (but **not** 0x06).
- **Gaps and flush.** Unchanged regions are emitted as copies, leaving holes
  where new or literal bytes belong. Each hole is recorded as a gap. The single
  FLUSH op walks the gap list in registration order and pours the opcode
  stream's remaining literal bytes into the holes. FLUSH occurs once, late in
  the program.

### Reconstruction outline

```
output = zeros(destination_size)
for each opcode:
    COPY / TCOPY  -> write source bytes at the write cursor (after any gap)
    STORE         -> remember a copy template
    ZFILL / FILL  -> write a zero or pattern run (after any gap)
    POKE*         -> delta-add into already-written bytes at the poke cursor
    FLUSH         -> backfill all gaps with literal bytes from the stream
    END           -> finished
```

---

## 10. Source validation

Each source entry carries a CRC (stored masked to 30 bits). An applier should
verify the on-disk source against this value before applying a record; a
mismatch indicates the wrong source version and the record should be skipped
rather than applied to incorrect data. The destination size is fixed by the
entry descriptor, so a wrong source can still yield a correctly-sized but
incorrect result if validation is skipped.

---

## 11. Open items

- The exact meaning of several reserved header words and the combine identifier.
- The 5-byte (count 4) VLI form is decodable but rarely, if ever, used.
- The precise CRC32 parameters used for source validation.
- Multi-source records (two or more sources, enabling the per-opcode source
  selector) are representable but uncommon.
