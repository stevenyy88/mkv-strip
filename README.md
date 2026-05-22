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
| `mkv-strip -l movie.mkv` | Inspect all tracks in an MKV (shorthand for `list`) |
| `mkv-strip list` | Inspect all tracks in an MKV (type, language, codec, flags) |
| `mkv-strip strip` | Remove audio/subtitle tracks by language or type |
| `mkv-strip keep` | Keep only specified track IDs, strip the rest |
| `mkv-strip extract` | Pull subtitle tracks out to `.srt` files |
| `mkv-strip add` | Inject an `.srt` file as a new subtitle track |

- **Pure Rust** — built on [`mkv-element`](https://crates.io/crates/mkv-element) for native EBML/Matroska parsing
- **No dependencies** — no FFmpeg, no mkvmerge, no runtime required
- **Tiny binary** — ~1.8 MB (Linux), ~2.2 MB (Windows)
- **Cross-platform** — Linux x64 & Windows x64 binaries available

## 🚀 Quick Start

### Download

Grab the latest binary from the [`binaries/`](binaries/) directory or build from source.

| File | Platform | Size | SHA256 |
|------|----------|------|--------|
| [`binaries/mkv-strip-linux-x64`](binaries/mkv-strip-linux-x64) | Linux (x86-64) | ~1.8 MB | `048f4d5fcf84e1db9bdf35601f087ff66e755faf0c3369f0f9f99d8d3a103227` |
| [`binaries/mkv-strip-windows-x64.exe`](binaries/mkv-strip-windows-x64.exe) | Windows (x86-64) | ~2.2 MB | `ce578c3987aca8d42abc3b00cf0a52538f944ac21889cab6a1bec0e3c8625e22` |

### Verify Download Authenticity

After downloading, verify the SHA-256 checksum to confirm the file hasn't been tampered with:

**Linux / macOS:**
```bash
sha256sum mkv-strip-linux-x64
# Expected: 048f4d5fcf84e1db9bdf35601f087ff66e755faf0c3369f0f9f99d8d3a103227
```

**Windows (PowerShell):**
```powershell
Get-FileHash .\mkv-strip-windows-x64.exe -Algorithm SHA256
# Expected: CE578C3987ACA8D42ABC3B00CF0A52538F944AC21889CAB6A1BEC0E3C8625E22
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
  # │ Type      │ Lang │ Flags            │ Name │ Codec
────┼───────────┼──────┼──────────────────┼──────┼──────────────
  1 │ video     │ und  │ enabled          │      │ V_MPEG4/ISO/AVC
  2 │ audio     │ eng  │ default, enabled │      │ A_AC3
  3 │ audio     │ jpn  │ enabled          │      │ A_AC3
  4 │ subtitle  │ eng  │ default, enabled │      │ S_TEXT/UTF8
  5 │ subtitle  │ spa  │ enabled          │      │ S_TEXT/UTF8
```

### Keep tracks by ID (shorthand)

Use `-k` to keep only specific track IDs and strip the rest. Track IDs are the `#` numbers from `list`:

```bash
# Shorthand with -k
mkv-strip -k 1,2,4 --keep-input movie.mkv --keep-output movie_stripped.mkv

# Or use the full command
mkv-strip keep -i movie.mkv -o movie_stripped.mkv --keep 1,2,4
```

This keeps only tracks #1, #2, and #4, and strips all others — useful for quickly trimming audio or subtitle tracks you don't need.

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
```

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
- **strip** — Two-pass: metadata scan first, then full re-read with block-level track filtering
- **keep** — Same two-pass approach as strip, but selects tracks by ID instead of language
- **add** — Parses SRT timestamps, converts to MKV segment ticks, builds SimpleBlock elements, appends new TrackEntry + clusters

## ⚠️ Limitations

- **Memory** — Clusters are loaded into memory during processing; very large files may use significant RAM
- **SeekHead / Cues** — Dropped from output; most players rebuild these automatically
- **Multi-segment files** — Not yet supported (rare in practice)
- **Track renumbering** — Track numbers are preserved as-is
- **Add command** — Always writes `S_TEXT/UTF8` codec; SRT positioning tags are not preserved

## 📜 License

MIT — Created by [Digital Futures Consultancy LLP (Singapore)](https://digitalfutures.asia)