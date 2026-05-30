<div align="center">

# 🎬 mkv-strip

**Strip, extract & add subtitles in MKV files. No FFmpeg needed.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)

A tiny, fast, single-binary CLI tool written in pure Rust that reads and writes MKV files natively — no external dependencies required.

</div>

---

## ✨ Features

| Command | Description |
|---------|-------------|
| `mkv-strip -l movie.mkv` | Inspect all tracks (shorthand for `list`) |
| `mkv-strip list` | Inspect all tracks in an MKV (type, language, codec, flags) |
| `mkv-strip flags -i movie.mkv --set-forced 3` | **Modify track flags in-place** (no output file needed) |
| `mkv-strip strip -k 1,2,4` | Keep only specified track IDs, strip the rest |
| `mkv-strip strip --set-forced 3` | Modify track flags (default, forced, enabled) |
| `mkv-strip strip` | Remove audio/subtitle tracks by language or type |
| `mkv-strip extract` | Pull subtitle tracks out to `.srt` files |
| `mkv-strip add --hearing-impaired` | Add subtitles with full flag support |

- **Pure Rust** — built on [`mkv-element`](https://crates.io/crates/mkv-element) for native EBML/Matroska parsing
- **No dependencies** — no FFmpeg, no mkvmerge, no runtime required
- **Low memory** — `strip` and `keep` stream clusters through memory, not the entire file
- **Tiny binary** — ~1.9 MB (Linux), ~2.3 MB (Windows)
- **Cross-platform** — Linux x64 & Windows x64 binaries available
- **Full flag support** — display and modify all Matroska track flags (default, forced, enabled, hearing-impaired, visual-impaired, descriptions, original, commentary)

## 🚀 Quick Start

### Download

Grab the latest binary from the [`binaries/`](binaries/) directory or build from source.

| File | Platform | Size | SHA256 |
|------|----------|------|--------|
| [`binaries/mkv-strip-linux-x64`](binaries/mkv-strip-linux-x64) | Linux (x86-64) | ~1.9 MB | `210b1113dd17bc8643bc0b3ac6795b9d6afc5ce6f899093a29aa37690e79419b` |
| [`binaries/mkv-strip-windows-x64.exe`](binaries/mkv-strip-windows-x64.exe) | Windows (x86-64) | ~2.3 MB | `657e55f8e1d3e5f691b24ea3f5d0719f0b178fb4d62909c1194869e64faf8feb` |

### Verify Download Authenticity

After downloading, verify the SHA-256 checksum to confirm the file hasn't been tampered with:

**Linux / macOS:**
```bash
sha256sum mkv-strip-linux-x64
# Expected: 210b1113dd17bc8643bc0b3ac6795b9d6afc5ce6f899093a29aa37690e79419b
```

**Windows (PowerShell):**
```powershell
Get-FileHash .\mkv-strip-windows-x64.exe -Algorithm SHA256
# Expected: 657E55F8E1D3E5F691B24EA3F5D0719F0B178FB4D62909C1194869E64FAF8FEB
```

If the hash doesn't match, **do not run the binary** — re-download it from this repository.

Full checksum reference: [`binaries/SHA256.md`](binaries/SHA256.md)

### List tracks (shorthand)

```bash
# Quick shorthand with -l
mkv-strip -l movie.mkv
```

Or use the full command:
```bash
mkv-strip list movie.mkv
```
```
  # │ Type      │ Lang │ Flags                        │ Name          │ Codec
────┼───────────┼──────┼──────────────────────────────┼───────────────┼──────────────
  1 │ video     │ und  │ enabled, default             │               │ V_MPEG4/ISO/AVC
  2 │ audio     │ eng  │ enabled, default             │               │ A_AC3
  3 │ audio     │ jpn  │ enabled                     │               │ A_AC3
  4 │ subtitle  │ eng  │ enabled, default, forced    │               │ S_TEXT/UTF8
  5 │ subtitle  │ spa  │ enabled                     │               │ S_TEXT/UTF8
```

### Keep tracks by ID

Use `-k` on the `strip` subcommand to keep only specific track IDs. Track IDs are the `#` numbers from `list`:

```bash
# Keep only video (1) and English audio (2)
mkv-strip strip -k 1,2 -i movie.mkv -o movie_stripped.mkv
```

This keeps only tracks #1 and #2, and strips all others — useful for quickly trimming audio or subtitle tracks you don't need.

### Modify track flags

Use `--set-*` and `--clear-*` options on the `strip` command to change track flags by track ID:

```bash
# Set subtitle track 4 as forced (e.g. forced narrative subtitles)
mkv-strip strip -i movie.mkv -o out.mkv --set-forced 4

# Clear default flag from audio track 2 and set it on track 3
mkv-strip strip -i movie.mkv -o out.mkv --clear-default 2 --set-default 3

# Disable a track without removing it
mkv-strip strip -i movie.mkv -o out.mkv --clear-enabled 5
```

Available flag options:
| Option | Description |
|---------------------|
| `--set-default <ids>` | Set tracks as default |
| `--clear-default <ids>` | Clear default flag from tracks |
| `--set-forced <ids>` | Set tracks as forced |
| `--clear-forced <ids>` | Clear forced flag from tracks |
| `--set-enabled <ids>` | Enable tracks |
| `--clear-enabled <ids>` | Disable tracks |

> **Note:** Track flags can also be combined with `--keep`, `--no-audio`, etc.

### Modify track flags in-place

Use the `flags` command to modify track flags **directly on the original file** — no output file needed, no re-encoding, instant:

```bash
# Set subtitle track 4 as forced
mkv-strip flags -i movie.mkv --set-forced 4

# Clear default flag from audio track 2 and set it on track 3
mkv-strip flags -i movie.mkv --clear-default 2 --set-default 3

# Disable a track without removing it
mkv-strip flags -i movie.mkv --clear-enabled 5

# Multiple operations at once
mkv-strip flags -i movie.mkv --set-default 3 --set-forced 4 --clear-default 2
```

Available options (same as `strip` flags):
| Option | Description |
|---------------------|
| `--set-default <ids>` | Set tracks as default |
| `--clear-default <ids>` | Clear default flag from tracks |
| `--set-forced <ids>` | Set tracks as forced |
| `--clear-forced <ids>` | Clear forced flag from tracks |
| `--set-enabled <ids>` | Enable tracks |
| `--clear-enabled <ids>` | Disable tracks |

> **How it works:** The `flags` command modifies the file in-place by overwriting only the flag bytes in the EBML structure. Since flag values are fixed-size integers (0 or 1), this is instant and doesn't require re-encoding or creating a new file. If a flag element doesn't exist yet, it falls back to a full rewrite (still replaces the original file).

### Strip tracks by language

Keep only English and Japanese audio:
```bash
mkv-strip strip -i movie.mkv -o movie_stripped.mkv --keep-audio eng,jpn
```

Remove specific subtitle languages:
```bash
mkv-strip strip -i movie.mkv -o movie_stripped.mkv --remove-subtitle spa
```

Remove all subtitles:
```bash
mkv-strip strip -i movie.mkv -o movie_stripped.mkv --no-subtitle
```

### Extract subtitles to SRT

```bash
# Extract all subtitle tracks
mkv-strip extract -i movie.mkv

# Filter by language
mkv-strip extract -i movie.mkv --lang eng,spa

# Filter by track number
mkv-strip extract -i movie.mkv -t 3,4

# Custom output directory
mkv-strip extract -i movie.mkv -o ./subs
```

Output files are named like `movie.4.eng.srt`, `movie.5.spa.English_SDH.srt`.

### Add an SRT subtitle track

```bash
# Basic
mkv-strip add -i movie.mkv -s subs.srt -o movie_with_subs.mkv

# With language and name
mkv-strip add -i movie.mkv -s subs.srt --lang eng --name "English (SDH)"

# Forced subtitle track
mkv-strip add -i movie.mkv -s forced.srt --lang eng --forced

# Set as default subtitle
mkv-strip add -i movie.mkv -s subs.srt --lang eng --default

# BCP-47 language code
mkv-strip add -i movie.mkv -s subs.srt --lang-bcp47 en

# Hearing-impaired subtitles (SDH)
mkv-strip add -i movie.mkv -s subs.srt --lang eng --name "English (SDH)" --hearing-impaired

# Commentary track
mkv-strip add -i movie.mkv -s subs.srt --lang eng --name "Director Commentary" --commentary
```

Available flag options for `add`:
| Option | Description |
|--------|
| `--default` | Set as default track |
| `--forced` | Set as forced track |
| `--hearing-impaired` | Mark as suitable for hearing-impaired users |
| `--visual-impaired` | Mark as suitable for visually-impaired users |
| `--descriptions` | Mark as text descriptions of video content |
| `--original` | Mark as original language track |
| `--commentary` | Mark as commentary track |

## 🛠 Build from Source

```bash
git clone https://github.com/stevenyy88/mkv-strip.git
cd mkv-strip
cargo build --release
# Binary at target/release/mkv-strip
```

Cross-compile for Windows x64 (requires [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)):
```bash
cargo zigbuild --release --target x86_64-pc-windows-gnu
# Binary at target/x86_64-pc-windows-gnu/release/mkv-strip.exe
```

> **Note:** To embed the application icon in the Windows exe, you also need `mingw-w64`'s `windres` and `ar` on your `PATH`. Without them, the exe works fine — it just won't have a custom icon.

## 📋 Supported Subtitle Codecs

| Extraction | Add |
|-----------|-----|
| `S_TEXT/UTF8` → `.srt` | `.srt` → `S_TEXT/UTF8` |
| `S_TEXT/SSA` → `.srt` | |
| `S_TEXT/ASS` → `.srt` | |

Image-based subtitles (VobSub `S_VOBSUB`, HDMV PGS) are not supported for extraction.

## ⚙️ How It Works

- **list / extract** — Uses `MatroskaView` to parse metadata without loading cluster data into memory
- **flags** — Modifies flag bytes in-place by locating EBML element positions and overwriting only the value bytes (instant, no re-encode); falls back to full rewrite if a flag element needs to be inserted
- **strip** — Two-pass: metadata scan first (via `MatroskaView`, lightweight), then streaming cluster processing with block-level track filtering. When all tracks are kept, clusters are raw-copied with zero decode/encode (~3 MB RAM). When some tracks are removed, clusters are parsed one at a time (~20 MB RAM peak). Can also modify track flags (default, forced, enabled).
- **strip -k** — Same two-pass approach, but selects tracks by ID instead of language
- **add** — Parses SRT timestamps, converts to MKV segment ticks, builds SimpleBlock elements, appends new TrackEntry + clusters. Writes a **SeekHead** at the Segment start (per RFC 9559 §6.3) and deduplicated **Cues** for fast seeking.

## 🧪 RFC 9559 Compliance

mkv-strip is validated against the [IETF Matroska test suite](https://github.com/ietf-wg-cellar/matroska-test-files) (8 official test files) plus custom round-trip tests.

Run the test suite:
```bash
git submodule update --init   # fetch IETF test files (one-time)
cargo test
```

| Test | What it verifies |
|------|------------------|
| `test_list_all_ietf_files` | `list` parses all IETF test files (1–3, 5–6, 8) |
| `test_strip_keep_video_audio` | Strip removes correct track types |
| `test_strip_roundtrip_reparsable` | Stripped output is valid, re-parsable MKV |
| `test_strip_by_language` | Language-based track filtering |
| `test_add_srt_creates_subtitle_track` | SRT → subtitle track creation |
| `test_add_extract_roundtrip` | Add → extract preserves subtitle text |
| `test_add_produces_seekhead_and_cues` | SeekHead + Cues present (RFC §6.3, §22) |
| `test_cues_deduplicated` | CuePoints sorted by time, deduplicated per cluster |
| `test_cluster_timestamps_monotonic` | Cluster timestamps in correct order |
| `test_flags_inplace` | In-place flag modification works |
| `test_srt_rectify_renumber` | Non-sequential indices rectified to 1, 2, 3… |
| `test_srt_rectify_zero_duration` | Zero/near-zero duration fixed to 200ms minimum |
| `test_srt_rectify_overlap` | Overlapping subtitles trimmed (100ms gap) |
| `test_srt_rectify_empty_text` | Empty/whitespace-only entries removed |
| `test_srt_rectify_flexible_timestamp` | Accepts `.` separator and 1–2 digit ms |
| `test_srt_rectify_extract_roundtrip` | Rectified SRT survives add → extract round-trip |

## ⚠️ Limitations

- **Memory** — Peak RAM is ~3-20 MB regardless of input file size. When all tracks are kept, clusters are raw-copied (no decode/encode). When some tracks are removed, clusters are parsed and filtered one at a time. `add`, `extract`, and `flags` commands also use minimal memory.
- **SeekHead / Cues** — The `add` command writes SeekHead and Cues (RFC 9559 compliant). The `strip` command does **not** rewrite Cues — most players rebuild these automatically.
- **Multi-segment files** — Not yet supported (rare in practice)
- **Track renumbering** — Track numbers are preserved as-is
- **Add command** — Always writes `S_TEXT/UTF8` codec; SRT positioning tags are not preserved
- **SRT validation** — Both `add` and `extract` validate and rectify SRT files automatically (see below)

### SRT Validation & Rectification

Both `add` and `extract` commands run an automatic SRT validation and rectification pass. Issues are fixed in-memory and a report is printed.

| Issue | Fix |
|-------|-----|
| Non-sequential indices (e.g. 5, 99) | Renumbered to 1, 2, 3… |
| Zero or near-zero duration (< 200ms) | Extended to 200ms minimum |
| End time ≤ start time | Set to start + 200ms |
| Overlapping subtitles | Previous end trimmed to next start − 100ms |
| Empty or whitespace-only text | Entry removed |
| Flexible timestamps (`.` or `,`, 1–3 digit ms) | Normalized to `HH:MM:SS,mmm` |

## 📜 License

MIT — Created by [Digital Futures Consultancy LLP (Singapore)](https://digitalfutures.asia)