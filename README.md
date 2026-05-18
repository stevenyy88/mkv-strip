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
| `mkv-strip list` | Inspect all tracks in an MKV (type, language, codec, flags) |
| `mkv-strip strip` | Remove audio/subtitle tracks by language or type |
| `mkv-strip extract` | Pull subtitle tracks out to `.srt` files |
| `mkv-strip add` | Inject an `.srt` file as a new subtitle track |

- **Pure Rust** — built on [`mkv-element`](https://crates.io/crates/mkv-element) for native EBML/Matroska parsing
- **No dependencies** — no FFmpeg, no mkvmerge, no runtime required
- **Tiny binary** — ~1.8 MB (Linux), ~2.2 MB (Windows)
- **Cross-platform** — Linux x64 & Windows x64 binaries available

## 🚀 Quick Start

### Download

Grab the latest binary from [Releases](https://github.com/stevenyy88/mkv-strip/releases) or build from source.

### List tracks

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

### Strip tracks

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
- **add** — Parses SRT timestamps, converts to MKV segment ticks, builds SimpleBlock elements, appends new TrackEntry + clusters

## ⚠️ Limitations

- **Memory** — Clusters are loaded into memory during processing; very large files may use significant RAM
- **SeekHead / Cues** — Dropped from output; most players rebuild these automatically
- **Multi-segment files** — Not yet supported (rare in practice)
- **Track renumbering** — Track numbers are preserved as-is
- **Add command** — Always writes `S_TEXT/UTF8` codec; SRT positioning tags are not preserved

## 📜 License

MIT — Created by [Digital Futures Consultancy LLP (Singapore)](https://digitalfutures.asia)
