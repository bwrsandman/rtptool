# rtptool

Inspect, extract, and apply RTPatch `.rtp` binary patch files.

> Licensed under [MIT](LICENSE). The RTPatch format is undocumented. The binary layout, codec, and opcode semantics were
> reverse-engineered with Claude (Anthropic). See [SPEC.md](SPEC.md) for the full format description.

## Build

```
cargo build --release
```

Binary: `target/release/rtptool`

---

## Commands

### `inspect` — show patch summary

```
rtptool inspect <file.rtp> [--verbose]
```

```
$ rtptool inspect update.rtp
File   : update.rtp
Version: 2.09 (0x0209)
Flags  : 0x82f4
Payload: 9447947 bytes
ExtraMd: true
Dirs   : 52
Records: 17

  MODIFY : 12
  NEW    : 5
```

`--verbose` lists every record with sizes and diff lengths.

---

### `list` — table of all file records

```
rtptool list <file.rtp> [--type modify|new|rename|delete|mkdir|all]
```

```
$ rtptool list update.rtp
type      dest_size   diff_size  filename
------------------------------------------------------------
MODIFY      1048576       24576  app.exe
MODIFY       524288       16384  renderer.dll
MODIFY        65536        1024  config.ini
NEW          131072              readme.txt
...
```

Filter to a specific record type:

```
$ rtptool list update.rtp --type new
```

---

### `apply` — apply MODIFY patches

```
rtptool apply <file.rtp> --source <dir> --output <dir> [options]
```

Reads source files from `--source`, writes patched versions to `--output`. Source subdirectory structure is preserved.

```
$ rtptool apply update.rtp --source ./v1.0 --output ./v1.1

Applying 12 MODIFY patch(es):
  source : ./v1.0
  output : ./v1.1

  [OK]    engine.dll  446549 B -> 446549 B  (./v1.1/engine.dll)
  [OK]    game.exe  8500623 B -> 9447947 B  (./v1.1/game.exe)
  [SKIP]  driver.dll -- source not found: ./v1.0/driver.dll
  [SKIP]  loader.dll -- checksum mismatch (...); use --no-checksum to apply anyway

Done: 10 patched, 2 skipped, 0 errors
```

**Options:**

| Flag | Effect |
|---|---|
| `--file <name>` | Only patch files whose name contains `<name>` (case-insensitive substring) |
| `--out-file <path>` | Write single-file output to this exact path (requires `--file` to match one record) |
| `--no-checksum` | Skip source-file checksum validation |

**Patch a single file:**

```
$ rtptool apply update.rtp --source ./v1.0 --output ./v1.1 --file game.exe
```

**Patch a single file to a specific path:**

```
$ rtptool apply update.rtp \
    --source ./v1.0 \
    --output /tmp \
    --file game.exe \
    --out-file ./game_patched.exe
```

**Skip checksum** (use when source files have been modified):

```
$ rtptool apply update.rtp --source ./v1.0 --output ./v1.1 --no-checksum
```

---

### `extract` — extract compressed diffs or NEW-file metadata

**Extract raw compressed diffs** (for debugging or re-use):

```
$ rtptool extract update.rtp --out ./diffs
Extracting 12 compressed diffs to ./diffs/
  game.exe -> ./diffs/game.exe.diff (6787248 B)
  engine.dll -> ./diffs/engine.dll.diff (19340 B)
  ...
```

**List NEW records** (files embedded as new, no diff data):

```
$ rtptool extract update.rtp --new-only
NEW records (3): no inline data in patch
  'readme.txt'  size=131,072 bytes
  'support.dll'  size=204,800 bytes
  ...
Metadata -> ./.new_files.txt
```

**Extract only a specific file's diff:**

```
$ rtptool extract update.rtp --out ./diffs --file engine.dll
```

---

## Error messages

| Message | Meaning |
|---|---|
| `source not found: ./v1.0/foo.dll` | Source file missing from `--source` directory |
| `checksum mismatch (expected 0x…, got 0x…)` | Source file checksum doesn't match — wrong version; pass `--no-checksum` to apply anyway |
| `bad diff magic: 0x… (want 0xB59C)` | Corrupt or truncated patch file |
| `unknown opcode 0x… at stream offset …` | Decompressed diff is corrupt |

---

## Library

Add to `Cargo.toml`:

```toml
rtptool = { path = "path/to/rtptool" }
```

```rust
use rtptool::{parse, patch_file};
use std::fs;

fn main() -> Result<(), rtptool::RtpError> {
    let data = fs::read("update.rtp")?;
    let patch = parse(data)?;

    println!("{} records", patch.records.len());

    let src = fs::read("v1.0/game.exe")?;

    for rec in patch.records.iter().filter(|r| r.has_diff()) {
        if rec.filename.contains("game.exe") {
            let patched = patch_file(&patch, rec, &src, true)?;
            fs::write("v1.1/game.exe", &patched)?;
        }
    }

    Ok(())
}
```

**API:**

```rust
// Parse a .rtp file
pub fn parse(data: Vec<u8>) -> Result<RtpPatch, RtpError>;

// Decompress and apply one MODIFY record
pub fn patch_file(
    patch: &RtpPatch,
    record: &FileRecord,
    src_data: &[u8],
    check_sum: bool,
) -> Result<Vec<u8>, RtpError>;

// 31-bit rolling checksum (w1 field in entry descriptors)
pub fn checksum_w1(data: &[u8]) -> u32;

// 30-bit rolling checksum (w2 field, used for source validation)
pub fn checksum_w2(data: &[u8]) -> u32;
```
