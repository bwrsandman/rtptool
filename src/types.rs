/// Record type nibble from bits 15-12 of rec_hdr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    Eof,
    Rename,
    New,
    Modify,
    Mkdir,
    Delete,
    Unknown(u8),
}

impl RecordType {
    pub fn from_nibble(n: u8) -> Self {
        match n {
            1 => Self::Eof,
            2 => Self::Rename,
            3 => Self::New,
            4 => Self::Modify,
            5 => Self::Mkdir,
            6 => Self::Delete,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eof => "EOF",
            Self::Rename => "RENAME",
            Self::New => "NEW",
            Self::Modify => "MODIFY",
            Self::Mkdir => "MKDIR",
            Self::Delete => "DELETE",
            Self::Unknown(_) => "UNKNOWN",
        }
    }
}

/// Source or destination file version descriptor (§6.3).
/// 10-byte checksum block layout: [b0][b1][w1: 4 LE][w2: 4 LE]
#[derive(Debug, Clone, Default)]
pub struct EntryDescriptor {
    pub file_size: u32,
    /// Weak length accumulator mod 31.
    pub b0: u8,
    /// Weak length accumulator mod 30.
    pub b1: u8,
    /// 31-bit rolling checksum (rotl8 within 31 bits), top bit masked.
    pub w1: u32,
    /// 30-bit rolling checksum (rotl8 within 30 bits), top 2 bits masked.
    pub w2: u32,
}

#[derive(Debug, Clone)]
pub struct RtpHeader {
    pub version: u16,
    pub flags: u16,
    pub ext_type_flags: u32,
    pub option_flags: u16,
    pub patch_total_size: u32,
    pub default_attrs: u16,
    pub cmd_flags: u16,
    pub combine_mode_id: Option<u32>,
    /// (ext_type_flags >> 16) & 1 — extra timestamps + alt-path in entries.
    pub extra_mode: bool,
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub rec_type: RecordType,
    pub rec_flags: u16,
    pub filename: String,
    pub src_path: Option<String>,
    pub dst_path: Option<String>,
    pub file_mod_flags: u16,
    /// Destination file size (from first dst entry for MODIFY, first src entry for NEW).
    pub new_file_size: u32,
    /// Byte offset within the raw RTP data where the compressed diff starts.
    pub patch_data_offset: usize,
    /// Byte length of the compressed diff (0 for non-MODIFY records).
    pub patch_data_size: usize,
    /// Number of source file versions referenced by this record.
    pub src_count: i64,
    pub src_entries: Vec<EntryDescriptor>,
    pub dst_entries: Vec<EntryDescriptor>,
}

impl FileRecord {
    pub fn has_diff(&self) -> bool {
        matches!(self.rec_type, RecordType::Modify) && self.patch_data_size > 0
    }
}

/// Parsed RTP patch file.
pub struct RtpPatch {
    pub header: RtpHeader,
    pub dirs: Vec<String>,
    pub records: Vec<FileRecord>,
    /// Raw bytes of the .rtp file, kept for extracting compressed diffs.
    pub raw: Vec<u8>,
}
