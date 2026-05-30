pub mod apply;
pub mod codec;
pub mod error;
pub mod parser;
pub mod reader;
pub mod types;

pub use error::RtpError;
pub use parser::parse;
pub use types::{EntryDescriptor, FileRecord, RecordType, RtpHeader, RtpPatch};

/// 31-bit rolling checksum (w1).
/// Each byte: w1 = rotl8(w1 ^ c) within 31 bits.
pub fn checksum_w1(data: &[u8]) -> u32 {
    let mut w: u32 = 0;
    for &c in data {
        let t = w ^ c as u32;
        w = ((t << 8) | (t >> 23)) & 0x7FFF_FFFF;
    }
    w
}

/// 30-bit rolling checksum (w2).
/// Each byte: w2 = rotl8(w2 ^ c) within 30 bits.
/// This is the value stored in entry descriptors (block10[6..10] & 0x3FFFFFFF).
pub fn checksum_w2(data: &[u8]) -> u32 {
    let mut w: u32 = 0;
    for &c in data {
        let t = w ^ c as u32;
        w = ((t << 8) | (t >> 22)) & 0x3FFF_FFFF;
    }
    w
}

/// High-level: decompress and apply a MODIFY record to a source file.
///
/// Returns the patched bytes. Validates the source checksum if `check_sum` is true.
pub fn patch_file(
    patch: &RtpPatch,
    record: &FileRecord,
    src_data: &[u8],
    check_sum: bool,
) -> Result<Vec<u8>, RtpError> {
    if check_sum
        && let Some(e) = record.src_entries.first().filter(|e| e.w2 != 0)
    {
        let actual = checksum_w2(src_data);
        if actual != e.w2 {
            return Err(RtpError::ChecksumMismatch {
                filename: record.filename.clone(),
                expected: e.w2,
                actual,
            });
        }
    }

    let opcodes = codec::decompress(
        &patch.raw,
        record.patch_data_offset,
        record.patch_data_size,
        (record.new_file_size as usize * 4).max(0x40_0000),
    )
    .map_err(|e| RtpError::DecompressError {
        filename: record.filename.clone(),
        detail: e,
    })?;

    let mut result = apply::apply(src_data, &opcodes, record.new_file_size as usize, record.src_count)
        .map_err(|e| match e {
            RtpError::ApplyError { detail, .. } => RtpError::ApplyError {
                filename: record.filename.clone(),
                detail,
            },
            other => other,
        })?;

    result.truncate(record.new_file_size as usize);
    Ok(result)
}
