# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-05-22

### Added

- **`-l` / `--list` shorthand** — List tracks without typing the `list` subcommand
  - `mkv-strip -l movie.mkv` is equivalent to `mkv-strip list movie.mkv`
- **`-k` / `--keep` shorthand** — Keep only specified track IDs and strip the rest
  - `mkv-strip -k 1,2,4 --keep-input movie.mkv --keep-output out.mkv`
- **`keep` subcommand** — Full command for keeping tracks by ID
  - `mkv-strip keep -i movie.mkv -o out.mkv --keep 1,2,4`
  - Track IDs are the `#` numbers shown by `list`

### Changed

- Running `mkv-strip` without arguments now shows help instead of an error

## [0.1.0] - 2026-05-18

### Added

- `list` command — inspect all tracks in an MKV file
- `strip` command — remove audio/subtitle tracks by language or type
- `extract` command — pull subtitle tracks out to `.srt` files
- `add` command — inject an `.srt` file as a new subtitle track
- Pure Rust implementation using `mkv-element` — no FFmpeg required
- Cross-platform support: Linux x64 & Windows x64