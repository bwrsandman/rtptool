use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RtpError {
    #[error("invalid magic: expected 4b 2a, got {0:02x?}")]
    InvalidMagic(Vec<u8>),

    #[error("unsupported version 0x{0:04x} (max 0x0209)")]
    UnsupportedVersion(u16),

    #[error("unsupported ext_type_flags 0x{0:08x}: low 3 bits must be zero")]
    UnsupportedFlags(u32),

    #[error("unexpected end of data at offset 0x{offset:08x}: need {needed} bytes, {available} available")]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },

    #[error("record {index}: {detail}")]
    InvalidRecord { index: usize, detail: String },

    #[error("source file not found for '{filename}': expected at {path}")]
    SourceNotFound { filename: String, path: PathBuf },

    #[error(
        "checksum mismatch for '{filename}': \
         patch expects 0x{expected:08x}, source computed 0x{actual:08x} \
         — wrong source version?"
    )]
    ChecksumMismatch {
        filename: String,
        expected: u32,
        actual: u32,
    },

    #[error("decompression error for '{filename}': {detail}")]
    DecompressError { filename: String, detail: String },

    #[error("patch apply error for '{filename}': {detail}")]
    ApplyError { filename: String, detail: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
