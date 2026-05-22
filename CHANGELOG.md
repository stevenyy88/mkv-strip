# Changelog

All notable changes to this project will be documented in this file.

## [0.2.1] - 2026-05-22

### Added

- **`-l` / `--list` shorthand** — List tracks without typing the `list` subcommand
  - `mkv-strip -l movie.mkv` is equivalent to `mkv-strip list movie.mkv`
- **`-k` / `--keep` option on `strip` subcommand** — Keep only specified track IDs and strip the rest
  - `mkv-strip strip -k 1,2,4 -i movie.mkv -o movie_stripped.mkv`
  - Track IDs are the `#` numbers shown by `list`
  - When `--keep` is used, all other strip filters (`--keep-audio`, `--remove-subtitle`, etc.) are ignored

### Changed

- Running `mkv-strip` without arguments now shows help instead of an error
- The `keep` subcommand and global `-k`/`--keep` shorthand from v0.2.0 have been replaced with `-k`/`--keep` as an option on `strip`

## [0.1.0] - 2026-05-18

### Added

- `list` command — inspect all tracks in an MKV file
- `strip` command — remove audio/subtitle tracks by language or type
- `extract` command — pull subtitle tracks out to `.srt` files
- `add` command — inject an `.srt` file as a new subtitle track
- Pure Rust implementation using `mkv-element` — no FFmpeg required
- Cross-platform support: Linux x64 & Windows x64