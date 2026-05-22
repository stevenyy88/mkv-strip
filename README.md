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
- **Tiny binary** — ~1.8 MB (Linux), ~2.2 MB (Windows)
- **Cross-platform** — Linux x64 & Windows x64 binaries available
- **Full flag support** — display and modify all Matroska track flags (default, forced, enabled, hearing-impaired, visual-impaired, descriptions, original, commentary)

## 🚀 Quick Start

### Download

Grab the latest binary from the [`binaries/`](binaries/) directory or build from source.

| File | Platform | Size | SHA256 |
|------|----------|------|--------|
| [`binaries/mkv-strip-linux-x64`](binaries/mkv-strip-linux-x64) | Linux (x86-64) | ~1.9 MB | `f92555cfde4ac30c220775ee379dd82a4f386a2ac4cd35f9fd274db1c2833025` |
| [`binaries/mkv-strip-windows-x64.exe`](binaries/mkv-strip-windows-x64.exe) | Windows (x86-64) | ~2.3 MB | `aab98bdaa01f4a1da0ee4f00c0d6b793b5050f29c440bdbe3ac43a7eef0d0789` |

### Verify Download Authenticity

After downloading, verify the SHA-256 checksum to confirm the file hasn't been tampered with:

**Linux / macOS:**
```bash
sha256sum mkv-strip-linux-x64
# Expected: f92555cfde4ac30c220775ee379dd82a4f386a2ac4cd35f9fd274db1c2833025
```

**Windows (PowerShell):**
```powershell
Get-FileHash .\mkv-strip-windows-x64.exe -Algorithm SHA256
# Expected: AAB98BDAA01F4A1DA0EE4F00C0D6B793B5050F29C440BDBE3AC43A7EEF0D0789
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
- **strip** — Two-pass: metadata scan first, then full re-read with block-level track filtering; can also modify track flags (default, forced, enabled)
- **strip -k** — Same two-pass approach, but selects tracks by ID instead of language
- **add** — Parses SRT timestamps, converts to MKV segment ticks, builds SimpleBlock elements, appends new TrackEntry + clusters

## ⚠️ Limitations

- **Memory** — Clusters are loaded into memory during processing; very large files may use significant RAM
- **SeekHead / Cues** — Dropped from output; most players rebuild these automatically
- **Multi-segment files** — Not yet supported (rare in practice)
- **Track renumbering** — Track numbers are preserved as-is
- **Add command** — Always writes `S_TEXT/UTF8` codec; SRT positioning tags are not preserved

## 📜 License

MIT — Created by [Digital Futures Consultancy LLP (Singapore)](https://digitalfutures.asia)