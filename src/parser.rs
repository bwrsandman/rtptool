use crate::error::RtpError;
use crate::reader::Reader;
use crate::types::{EntryDescriptor, FileRecord, RecordType, RtpHeader, RtpPatch};

type RecordData = (u16, u32, usize, usize, i64, Vec<EntryDescriptor>, Vec<EntryDescriptor>);

// Header flags (hdr.flags)
const FLAG_EXT_TYPE_PRESENT: u16 = 0x8000;
const FLAG_DIR_TABLE: u16        = 0x0200; // bit 1 of high byte => (flags >> 8) & 2

// cmd_flags
const CMD_FLAG_COMBINE_ID: u16 = 0x0004;

// rec_hdr bits (low 12 bits)
const REC_HAS_OPTION_FLAGS: u16 = 0x0002;
const REC_HAS_FILENAME: u16     = 0x0004;
const REC_HAS_DISK_IDX: u16     = 0x0080;
const REC_HAS_ATTRS: u16        = 0x0100;
const REC_HAS_PATHS: u16        = 0x0200;

// option_flags bits for seek/checksum
const OPT_HAS_SEEK: u16 = 0x00C0;

// CRC mask (30 bits as stored in entry descriptors)
const CRC_MASK: u32 = 0x3FFF_FFFF;

pub fn parse(data: Vec<u8>) -> Result<RtpPatch, RtpError> {
    let mut r = Reader::new(&data);

    let header = parse_header(&mut r)?;
    let dirs = parse_dir_table(&mut r, &header)?;
    let records = parse_records(&mut r, &header)?;

    Ok(RtpPatch { header, dirs, records, raw: data })
}

fn parse_header(r: &mut Reader<'_>) -> Result<RtpHeader, RtpError> {
    let magic = r.read_exact(2)?.to_vec();
    if magic != b"K*" {
        return Err(RtpError::InvalidMagic(magic));
    }

    let version = r.u16_le()?;
    if version > 0x0209 {
        return Err(RtpError::UnsupportedVersion(version));
    }

    let flags = r.u16_le()?;
    let ext_type_flags = if flags & FLAG_EXT_TYPE_PRESENT != 0 { r.u32_le()? } else { 0 };

    let option_flags = r.u16_le()?;

    if ext_type_flags & 7 != 0 {
        return Err(RtpError::UnsupportedFlags(ext_type_flags));
    }

    let extra_mode = (ext_type_flags >> 16) & 1 != 0;
    let patch_total_size = r.u32_le()?;
    let _reserved_a = r.u32_le()?;
    let default_attrs = r.u16_le()?;
    let _reserved_b = r.u16_le()?;
    let cmd_flags = r.u16_le()?;
    let combine_mode_id = if cmd_flags & CMD_FLAG_COMBINE_ID != 0 { Some(r.u32_le()?) } else { None };
    let _reserved_c = r.u32_le()?;

    Ok(RtpHeader {
        version,
        flags,
        ext_type_flags,
        option_flags,
        patch_total_size,
        default_attrs,
        cmd_flags,
        combine_mode_id,
        extra_mode,
    })
}

fn parse_dir_table(r: &mut Reader<'_>, hdr: &RtpHeader) -> Result<Vec<String>, RtpError> {
    if hdr.flags & FLAG_DIR_TABLE == 0 {
        return Ok(Vec::new());
    }
    let n = r.u16_le()? as usize;
    let mut dirs = Vec::with_capacity(n);
    for _ in 0..n {
        dirs.push(r.lp_string()?);
    }
    Ok(dirs)
}

fn parse_records(r: &mut Reader<'_>, hdr: &RtpHeader) -> Result<Vec<FileRecord>, RtpError> {
    let mut records = Vec::new();
    let mut idx = 0usize;

    loop {
        let rec_hdr = r.u16_le()?;
        let rec_type = RecordType::from_nibble(((rec_hdr >> 12) & 0xF) as u8);

        if rec_type == RecordType::Eof {
            break;
        }

        let mut option_flags = hdr.option_flags;
        if rec_hdr & REC_HAS_OPTION_FLAGS != 0 {
            option_flags = r.u16_le()?;
        }

        let filename = if rec_hdr & REC_HAS_FILENAME != 0 {
            r.lp_string()?
        } else {
            String::new()
        };

        if option_flags & OPT_HAS_SEEK != 0 {
            let _seek = r.vli()?;
            if hdr.extra_mode {
                let _chk = r.vli()?;
            }
        }

        if rec_hdr & REC_HAS_DISK_IDX != 0 {
            let _disk_idx = r.vli()?;
        }

        if rec_hdr & REC_HAS_ATTRS != 0 {
            let _attrs = r.u16_le()?;
        }

        let (src_path, dst_path) = if rec_hdr & REC_HAS_PATHS != 0 && rec_type != RecordType::Mkdir {
            (Some(r.lp_string()?), Some(r.lp_string()?))
        } else {
            (None, None)
        };

        if rec_type == RecordType::Mkdir {
            let _dir_u32 = r.u32_le()?;
            let _dir_u16 = r.u16_le()?;
        }

        let _metadata = r.read_exact(10)?;

        let (file_mod_flags, new_file_size, patch_data_offset, patch_data_size, src_count,
             src_entries, dst_entries) = if rec_type != RecordType::Mkdir {
            parse_record_data(r, rec_type, hdr.extra_mode)
                .map_err(|e| RtpError::InvalidRecord { index: idx, detail: e.to_string() })?
        } else {
            (0, 0, 0, 0, 0, Vec::new(), Vec::new())
        };

        records.push(FileRecord {
            rec_type,
            rec_flags: rec_hdr,
            filename,
            src_path,
            dst_path,
            file_mod_flags,
            new_file_size,
            patch_data_offset,
            patch_data_size,
            src_count,
            src_entries,
            dst_entries,
        });

        idx += 1;
    }

    Ok(records)
}

fn parse_record_data(
    r: &mut Reader<'_>,
    rec_type: RecordType,
    extra_mode: bool,
) -> Result<RecordData, RtpError> {
    match rec_type {
        RecordType::New => {
            let src_count = r.vli()?;
            let mut src_entries = Vec::with_capacity(src_count as usize);
            let mut new_file_size = 0u32;
            for _ in 0..src_count {
                let e = read_entry(r, extra_mode)?;
                if new_file_size == 0 {
                    new_file_size = e.file_size;
                }
                src_entries.push(e);
            }
            Ok((0, new_file_size, r.pos, 0, src_count, src_entries, Vec::new()))
        }

        RecordType::Modify => {
            let file_mod_flags = r.u16_le()?;
            let src_count = r.vli()?;
            let dst_count = r.vli()?;
            let _unk4 = r.u32_le()?;
            let skip_off = r.u32_le()? as usize;

            let src_entries = read_entries(r, src_count as usize, extra_mode)?;
            let dst_entries = read_entries(r, dst_count as usize, extra_mode)?;

            let new_file_size = dst_entries.first().map(|e| e.file_size).unwrap_or(0);
            let data_start = r.pos;
            r.read_exact(skip_off)?;

            Ok((file_mod_flags, new_file_size, data_start, skip_off, src_count,
                src_entries, dst_entries))
        }

        RecordType::Rename | RecordType::Delete => {
            let dst_count = r.vli()?;
            let _unk4 = r.u32_le()?;
            let skip_off = r.u32_le()? as usize;
            let dst_entries = read_entries(r, dst_count as usize, extra_mode)?;
            let new_file_size = dst_entries.first().map(|e| e.file_size).unwrap_or(0);
            let data_start = r.pos;
            r.read_exact(skip_off)?;
            Ok((0, new_file_size, data_start, skip_off, 0, Vec::new(), dst_entries))
        }

        _ => {
            let dst_count = r.vli()?;
            let _unk4 = r.u32_le()?;
            let skip_off = r.u32_le()? as usize;
            let dst_entries = read_entries(r, dst_count as usize, extra_mode)?;
            let new_file_size = dst_entries.first().map(|e| e.file_size).unwrap_or(0);
            let data_start = r.pos;
            r.read_exact(skip_off)?;
            Ok((0, new_file_size, data_start, skip_off, 0, Vec::new(), dst_entries))
        }
    }
}

fn read_entry(r: &mut Reader<'_>, extra_mode: bool) -> Result<EntryDescriptor, RtpError> {
    let block24 = r.read_exact(24)?;
    let block10 = r.read_exact(10)?;
    let file_size = u32::from_le_bytes(block24[16..20].try_into().unwrap());
    let crc32 = u32::from_le_bytes(block10[6..10].try_into().unwrap()) & CRC_MASK;
    if extra_mode {
        r.read_exact(8)?;
        r.lp_string()?;
    }
    Ok(EntryDescriptor { file_size, crc32 })
}

fn read_entries(
    r: &mut Reader<'_>,
    count: usize,
    extra_mode: bool,
) -> Result<Vec<EntryDescriptor>, RtpError> {
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(read_entry(r, extra_mode)?);
    }
    Ok(entries)
}
