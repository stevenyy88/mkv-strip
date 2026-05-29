# Changelog

All notable changes to this project will be documented in this file.

## [0.2.7] - 2026-05-29

### Changed

- **`add` command now uses streaming I/O** — existing clusters are raw-copied from input to output without decoding/re-encoding, then subtitle clusters are appended. Peak RAM drops from 2× file size (full decode + re-encode) to ~1 MB (bounded by subtitle cluster size).
  - Video/audio clusters: zero-copy passthrough (seek + 64KB buffer)
  - New subtitle clusters: built from SRT entries and written directly
  - Metadata (Info, Tracks, Attachments, Chapters): rewritten from MatroskaView with updated track entry
  - Tags: preserved from original

## [0.2.6] - 2026-05-29

### Changed

- **`extract` command now uses streaming I/O** — peak RAM is bounded by a single subtitle block (~a few KB) instead of loading entire clusters (video+audio+subtitles) into memory. For a 10 GB 1080p MKV with subtitles, peak RAM drops from potentially hundreds of MB to ~1 MB.
  - Only subtitle block data is read into memory; video and audio blocks are skipped via seek
  - Handles laced subtitle frames correctly (Xiph, EBML, fixed-size lacing)
  - BlockGroups are decoded for proper BlockDuration extraction

### Fixed

- **`extract` command no longer crashes on truncated MKV files** — if a cluster or element size extends beyond the actual file (common with incomplete downloads or improperly finalized files), the command now prints a warning and extracts all subtitles found up to that point instead of failing with `failed to fill whole buffer`

## [0.2.5] - 2026-05-23

### Changed

- **Fixed memory consumption for large MKV files** — peak RAM is now ~3-20 MB regardless of input file size (was: file size * 2 for `strip`/`keep` commands)
  - When ALL tracks are kept: raw byte-copy clusters with no decode/encode — ~3 MB peak RAM
  - When SOME tracks are removed: parse + filter clusters one at a time — ~20 MB peak RAM per cluster
  - Previous versions loaded the entire file into memory before writing, causing Windows "Insufficient system resources" (error 1450) on large files

## [0.2.4] - 2026-05-23

### Changed

- **Streaming writes for `strip` and `keep` commands** — clusters are now written one at a time instead of buffering all clusters in memory. Peak memory usage is now bounded by a single cluster (~1-2 MB for typical files) instead of the entire file. This fixes the Windows "Insufficient system resources" error (OS error 1450) when processing large MKV files.

## [0.2.3] - 2026-05-23

### Added

- **`flags` command** — modify track flags in-place on the original MKV file without creating a new file
  - `mkv-strip flags -i movie.mkv --set-forced 3 --clear-default 2`
  - Options: `--set-default`, `--clear-default`, `--set-forced`, `--clear-forced`, `--set-enabled`, `--clear-enabled`
  - All options accept comma-separated track IDs
  - Modifies the file in-place by overwriting only the flag bytes (no re-encode, instant)
  - Falls back to full rewrite if a flag element doesn't exist yet in the file

## [0.2.2] - 2026-05-22

### Added

- **Full track flag support** — all Matroska track flags are now displayed and configurable:
  - `enabled`, `default`, `forced` (already shown)
  - `hearing-impaired` — track is suitable for users with hearing impairments
  - `visual-impaired` — track is suitable for users with visual impairments
  - `descriptions` — track contains textual descriptions of video content (audio descriptions)
  - `original` — track is in the content's original language
  - `commentary` — track contains commentary
- **`strip` flag modification options** — set or clear track flags by track ID:
  - `--set-default <ids>` / `--clear-default <ids>`
  - `--set-forced <ids>` / `--clear-forced <ids>`
  - `--set-enabled <ids>` / `--clear-enabled <ids>`
- **`add` command flag options** — set track flags when adding subtitles:
  - `--hearing-impaired` — mark as hearing-impaired track
  - `--visual-impaired` — mark as visual-impaired track
  - `--descriptions` — mark as text descriptions track
  - `--original` — mark as original language track
  - `--commentary` — mark as commentary track

### Changed

- Track flags in `list` output now show all flags (previously only `default`, `forced`, `enabled`)
- Flag display order: `enabled`, `default`, `forced`, `hearing-impaired`, `visual-impaired`, `descriptions`, `original`, `commentary`

## [0.2.1] - 2026-05-22

### Added

- **`-l` / `--list` shorthand** — List tracks without typing the `list` subcommand
  - `mkv-strip -l movie.mkv` → equivalent to `mkv-strip list movie.mkv`
- **`-k` / `--keep` option on `strip` subcommand** — Keep only specified track IDs and strip the rest
  - `mkv-strip strip -k 1,2,4 -i movie.mkv -o movie_stripped.mkv`
  - Track IDs are the `#` numbers shown by `list`
- Running `mkv-strip` without arguments now shows help instead of an error

## [0.1.0] - 2026-05-18

### Added

- `list` command — inspect all tracks in an MKV file
- `strip` command — remove audio/subtitle tracks by language or type
- `extract` command — pull subtitle tracks out to `.srt` files
- `add` command — inject an `.srt` file as a new subtitle track
- Pure Rust implementation using `mkv-element` — no FFmpeg required
- Cross-platform support: Linux x64 & Windows x64