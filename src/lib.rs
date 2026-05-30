pub mod apply;
pub mod codec;
pub mod error;
pub mod parser;
pub mod reader;
pub mod types;

pub use error::RtpError;
pub use parser::parse;
pub use types::{EntryDescriptor, FileRecord, RecordType, RtpHeader, RtpPatch};

use crc32fast::Hasher;

/// Compute CRC32 of `data` masked to 30 bits (as stored in entry descriptors).
pub fn crc32_masked(data: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(data);
    const CRC_MASK: u32 = 0x3FFF_FFFF;
    h.finalize() & CRC_MASK
}

/// High-level: decompress and apply a MODIFY record to a source file.
///
/// Returns the patched bytes. Validates the source CRC if `check_crc` is true.
pub fn patch_file(
    patch: &RtpPatch,
    record: &FileRecord,
    src_data: &[u8],
    check_crc: bool,
) -> Result<Vec<u8>, RtpError> {
    if check_crc
        && let Some(e) = record.src_entries.first().filter(|e| e.crc32 != 0)
    {
        let actual = crc32_masked(src_data);
        if actual != e.crc32 {
            return Err(RtpError::CrcMismatch {
                filename: record.filename.clone(),
                expected: e.crc32,
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
