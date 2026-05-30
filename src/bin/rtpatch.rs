use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use rtptool::{FileRecord, RecordType, RtpError, RtpPatch};

#[derive(Parser)]
#[command(name = "rtptool", about = "Inspect, extract, and apply RTPatch (.rtp) files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show header info and record summary.
    Inspect {
        rtp_file: PathBuf,
        #[arg(short, long)]
        verbose: bool,
    },

    /// List all file records.
    List {
        rtp_file: PathBuf,
        /// Filter by record type: modify, new, rename, delete, mkdir, all (default: all).
        #[arg(short = 't', long, default_value = "all")]
        record_type: String,
    },

    /// Extract raw compressed diffs or NEW file metadata.
    Extract {
        rtp_file: PathBuf,
        /// Output directory.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Only extract this specific filename (substring match).
        #[arg(short, long)]
        file: Option<String>,
        /// Extract NEW record metadata only (no diff bytes to extract from NEW records).
        #[arg(long)]
        new_only: bool,
    },

    /// Apply MODIFY patches.
    Apply {
        rtp_file: PathBuf,
        /// Directory containing original (unpatched) source files.
        #[arg(short, long)]
        source: PathBuf,
        /// Directory to write patched files into.
        #[arg(short, long)]
        output: PathBuf,
        /// Only apply patch for this filename (substring match).
        #[arg(short, long)]
        file: Option<String>,
        /// For single-file mode: write output to this exact path instead of --output/<name>.
        #[arg(long)]
        out_file: Option<PathBuf>,
        /// Skip checksum validation of source files.
        #[arg(long)]
        no_checksum: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn load_patch(path: &Path) -> Result<RtpPatch, RtpError> {
    let data = fs::read(path).map_err(RtpError::Io)?;
    rtptool::parse(data)
}

fn filter_filename(rec: &FileRecord, filter: &Option<String>) -> bool {
    match filter {
        None => true,
        Some(f) => rec.filename.to_lowercase().contains(&f.to_lowercase()),
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Inspect { rtp_file, verbose } => {
            let p = load_patch(&rtp_file)?;
            let h = &p.header;

            println!("File   : {}", rtp_file.display());
            println!("Version: {}.{:02} (0x{:04x})", h.version >> 8, h.version & 0xFF, h.version);
            println!("Flags  : 0x{:04x}", h.flags);
            println!("Payload: {} bytes", h.patch_total_size);
            println!("ExtraMd: {}", h.extra_mode);
            println!("Dirs   : {}", p.dirs.len());
            println!("Records: {}", p.records.len());

            if verbose && !p.dirs.is_empty() {
                println!();
                for (i, d) in p.dirs.iter().enumerate() {
                    println!("  dir[{i}] = {d:?}");
                }
            }

            println!();
            let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
            for rec in &p.records {
                *counts.entry(rec.rec_type.as_str()).or_default() += 1;
            }
            for (t, n) in &counts {
                println!("  {t:7}: {n}");
            }

            if verbose {
                println!();
                for rec in &p.records {
                    let size = if rec.new_file_size > 0 {
                        format!("{:>10} B", rec.new_file_size)
                    } else {
                        " ".repeat(11)
                    };
                    let diff = if rec.patch_data_size > 0 {
                        format!(" diff={}", rec.patch_data_size)
                    } else {
                        String::new()
                    };
                    println!("  [{:6}] {size}{diff}  {}", rec.rec_type.as_str(), rec.filename);
                }
            }
        }

        Command::List { rtp_file, record_type } => {
            let p = load_patch(&rtp_file)?;
            let type_filter = record_type.to_lowercase();

            println!("{:<8} {:>10}  {:>10}  filename", "type", "dest_size", "diff_size");
            println!("{}", "-".repeat(60));

            for rec in p.records.iter().filter(|r| {
                type_filter == "all" || r.rec_type.as_str().to_lowercase() == type_filter
            }) {
                let size = if rec.new_file_size > 0 {
                    format!("{:>10}", rec.new_file_size)
                } else {
                    " ".repeat(10)
                };
                let diff = if rec.patch_data_size > 0 {
                    format!("{:>10}", rec.patch_data_size)
                } else {
                    " ".repeat(10)
                };
                println!("{:<8} {size}  {diff}  {}", rec.rec_type.as_str(), rec.filename);
            }
        }

        Command::Extract { rtp_file, out, file, new_only } => {
            let p = load_patch(&rtp_file)?;
            fs::create_dir_all(&out)?;

            if new_only {
                let records: Vec<_> = p.records.iter()
                    .filter(|r| r.rec_type == RecordType::New && filter_filename(r, &file))
                    .collect();
                println!("NEW records ({}): no inline data in patch", records.len());
                let meta = out.join(".new_files.txt");
                let mut lines = format!(
                    "# NEW files in {}\n# filename\texpected_size\n",
                    rtp_file.display()
                );
                for rec in &records {
                    println!("  {:>10} B  {}", rec.new_file_size, rec.filename);
                    lines.push_str(&format!("{}\t{}\n", rec.filename, rec.new_file_size));
                }
                fs::write(&meta, &lines)?;
                println!("Metadata -> {}", meta.display());
                return Ok(());
            }

            let records: Vec<_> = p.records.iter()
                .filter(|r| r.has_diff() && filter_filename(r, &file))
                .collect();

            println!("Extracting {} compressed diffs to {}/", records.len(), out.display());
            for rec in records {
                let safe = rec.filename.replace(['\\', '/'], "_");
                let dest = out.join(format!("{safe}.diff"));
                let diff = &p.raw[rec.patch_data_offset..rec.patch_data_offset + rec.patch_data_size];
                fs::write(&dest, diff)?;
                println!("  {} -> {} ({} B)", rec.filename, dest.display(), rec.patch_data_size);
            }
        }

        Command::Apply { rtp_file, source, output, file, out_file, no_checksum } => {
            let p = load_patch(&rtp_file)?;
            fs::create_dir_all(&output)?;

            let records: Vec<_> = p.records.iter()
                .filter(|r| r.has_diff() && filter_filename(r, &file))
                .collect();

            if records.is_empty() {
                println!("No MODIFY records match.");
                return Ok(());
            }

            if out_file.is_some() && records.len() > 1 {
                eprintln!("error: --out-file requires --file to match exactly one record");
                std::process::exit(1);
            }

            println!("Applying {} MODIFY patch(es):", records.len());
            println!("  source : {}", source.display());
            println!("  output : {}", output.display());
            println!();

            let mut ok = 0usize;
            let mut skip = 0usize;
            let mut err = 0usize;

            for rec in &records {
                let rel = rec.filename.replace('\\', std::path::MAIN_SEPARATOR_STR);
                let src_path = source.join(&rel);
                let dst_path = if let Some(ref of) = out_file {
                    of.clone()
                } else {
                    output.join(&rel)
                };

                if !src_path.exists() {
                    eprintln!("  [SKIP]  {} -- source not found: {}", rec.filename, src_path.display());
                    skip += 1;
                    continue;
                }

                let src_data = match fs::read(&src_path) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("  [ERR]   {} -- read source: {e}", rec.filename);
                        err += 1;
                        continue;
                    }
                };

                match rtptool::patch_file(&p, rec, &src_data, !no_checksum) {
                    Ok(patched) => {
                        if let Some(Err(e)) = dst_path.parent().map(fs::create_dir_all) {
                            eprintln!("  [ERR]   {} -- create dir: {e}", rec.filename);
                            err += 1;
                            continue;
                        }
                        match fs::write(&dst_path, &patched) {
                            Ok(()) => {
                                println!(
                                    "  [OK]    {}  {} B -> {} B  ({})",
                                    rec.filename, src_data.len(), patched.len(), dst_path.display()
                                );
                                ok += 1;
                            }
                            Err(e) => {
                                eprintln!("  [ERR]   {} -- write output: {e}", rec.filename);
                                err += 1;
                            }
                        }
                    }
                    Err(RtpError::ChecksumMismatch { filename, expected, actual }) => {
                        eprintln!(
                            "  [SKIP]  {filename} -- CRC mismatch \
                             (expected 0x{expected:08x}, got 0x{actual:08x}); \
                             use --no-checksum to apply anyway"
                        );
                        skip += 1;
                    }
                    Err(e) => {
                        eprintln!("  [ERR]   {} -- {e}", rec.filename);
                        err += 1;
                    }
                }
            }

            println!();
            println!("Done: {ok} patched, {skip} skipped, {err} errors");
            if err > 0 {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
