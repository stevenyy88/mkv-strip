use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, CommandFactory};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::PathBuf;

use bytes::Bytes;
use mkv_element::io::blocking_impl::{ReadElement, ReadFrom, WriteTo};
use mkv_element::prelude::*;
use mkv_element::view::MatroskaView;
use mkv_element::ClusterBlock;

// ---------------------------------------------------------------------------
// Track type constants (from Matroska spec)
// ---------------------------------------------------------------------------
const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 17;

fn track_type_name(tt: u64) -> &'static str {
    match tt {
        TRACK_TYPE_VIDEO => "video",
        TRACK_TYPE_AUDIO => "audio",
        TRACK_TYPE_SUBTITLE => "subtitle",
        3 => "complex",
        16 => "logo",
        18 => "buttons",
        32 => "control",
        33 => "metadata",
        _ => "unknown",
    }
}

/// Parse an EBML VInt from raw bytes and return the decoded integer value.
fn parse_vint_value(data: &[u8]) -> Option<u64> {
    if data.is_empty() {
        return None;
    }
    let first = data[0];
    if first == 0 {
        return None;
    }
    let leading_zeros = first.leading_zeros() as usize;
    if leading_zeros >= 8 {
        return None;
    }
    let vint_len = leading_zeros + 1;
    if data.len() < vint_len {
        return None;
    }
    if vint_len == 1 {
        Some((first & 0x7F) as u64)
    } else {
        let mut result: u64 = (first & (0xFF >> leading_zeros)) as u64;
        for &b in &data[1..vint_len] {
            result = (result << 8) | b as u64;
        }
        Some(result)
    }
}

/// Extract the track number from a raw SimpleBlock or Block byte sequence.
fn track_number_from_block(data: &[u8]) -> Option<u64> {
    parse_vint_value(data)
}

// ---------------------------------------------------------------------------
// TrackInfo — resolved from TrackEntry
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
struct TrackInfo {
    number: u64,
    #[allow(dead_code)]
    uid: u64,
    track_type: u64,
    codec_id: String,
    language: String,
    language_bcp47: Option<String>,
    name: Option<String>,
    flag_enabled: bool,
    flag_default: bool,
    flag_forced: bool,
    flag_hearing_impaired: bool,
    flag_visual_impaired: bool,
    flag_text_descriptions: bool,
    flag_original: bool,
    flag_commentary: bool,
}

impl TrackInfo {
    fn from_track_entry(te: &TrackEntry) -> Self {
        Self {
            number: *te.track_number,
            uid: *te.track_uid,
            track_type: *te.track_type,
            codec_id: te.codec_id.0.clone(),
            language: te.language.0.clone(),
            language_bcp47: te.language_bcp47.as_ref().map(|l| l.0.clone()),
            name: te.name.as_ref().map(|n| n.0.clone()),
            flag_enabled: *te.flag_enabled != 0,
            flag_default: *te.flag_default != 0,
            flag_forced: *te.flag_forced != 0,
            flag_hearing_impaired: te.flag_hearing_impaired.map_or(false, |f| *f != 0),
            flag_visual_impaired: te.flag_visual_impaired.map_or(false, |f| *f != 0),
            flag_text_descriptions: te.flag_text_descriptions.map_or(false, |f| *f != 0),
            flag_original: te.flag_original.map_or(false, |f| *f != 0),
            flag_commentary: te.flag_commentary.map_or(false, |f| *f != 0),
        }
    }

    fn lang_display(&self) -> String {
        match &self.language_bcp47 {
            Some(bcp) if bcp.to_ascii_lowercase() != self.language.to_ascii_lowercase() => {
                format!("{} [{}]", self.language, bcp)
            }
            Some(_) => self.language.clone(),
            None => self.language.clone(),
        }
    }

    fn flags_display(&self) -> String {
        let flags = [
            self.flag_enabled.then_some("enabled"),
            self.flag_default.then_some("default"),
            self.flag_forced.then_some("forced"),
            self.flag_hearing_impaired.then_some("hearing-impaired"),
            self.flag_visual_impaired.then_some("visual-impaired"),
            self.flag_text_descriptions.then_some("descriptions"),
            self.flag_original.then_some("original"),
            self.flag_commentary.then_some("commentary"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        if flags.is_empty() { "-".to_string() } else { flags }
    }
}

// ---------------------------------------------------------------------------
// Table formatting — dynamic column widths
// ---------------------------------------------------------------------------
struct TableColumn {
    header: String,
    width: usize,
}

struct TrackTable {
    columns: Vec<TableColumn>,
    rows: Vec<Vec<String>>,
}

impl TrackTable {
    fn build(track_infos: &[TrackInfo]) -> Self {
        let col_defs: Vec<(&str, Box<dyn Fn(&TrackInfo) -> String>)> = vec![
            ("#", Box::new(|t| t.number.to_string())),
            ("Type", Box::new(|t| track_type_name(t.track_type).to_string())),
            ("Lang", Box::new(|t| t.lang_display())),
            ("Flags", Box::new(|t| t.flags_display())),
            ("Name", Box::new(|t| t.name.clone().unwrap_or_default())),
            ("Codec", Box::new(|t| t.codec_id.clone())),
        ];

        let rows: Vec<Vec<String>> = track_infos
            .iter()
            .map(|t| col_defs.iter().map(|(_, f)| f(t)).collect())
            .collect();

        let columns: Vec<TableColumn> = col_defs
            .iter()
            .enumerate()
            .map(|(ci, (header, _))| {
                let header_w = header.len();
                let max_data_w = rows.iter().map(|r| r[ci].len()).max().unwrap_or(0);
                TableColumn {
                    header: header.to_string(),
                    width: header_w.max(max_data_w),
                }
            })
            .collect();

        TrackTable { columns, rows }
    }

    fn header_line(&self) -> String {
        let cells: Vec<String> = self
            .columns
            .iter()
            .map(|c| pad_right(&c.header, c.width))
            .collect();
        format!("  {}", cells.join(" │ "))
    }

    fn separator_line(&self) -> String {
        let cells: Vec<String> = self.columns.iter().map(|c| "─".repeat(c.width)).collect();
        format!("  {}", cells.join("─┼─"))
    }

    fn row_line(&self, row: &[String]) -> String {
        let cells: Vec<String> = self
            .columns
            .iter()
            .zip(row.iter())
            .map(|(c, v)| pad_right(v, c.width))
            .collect();
        format!("  {}", cells.join(" │ "))
    }
}

fn pad_right(s: &str, width: usize) -> String {
    if s.len() >= width {
        s.to_string()
    } else {
        let pad = width - s.len();
        format!("{}{}", s, " ".repeat(pad))
    }
}

// ---------------------------------------------------------------------------
// SRT subtitle handling
// ---------------------------------------------------------------------------

/// A single SRT subtitle entry.
#[derive(Debug, Clone)]
struct SrtEntry {
    index: u32,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

impl SrtEntry {
    fn format_timestamp(ms: u64) -> String {
        let h = ms / 3_600_000;
        let m = (ms % 3_600_000) / 60_000;
        let s = (ms % 60_000) / 1000;
        let frac = ms % 1000;
        format!("{:02}:{:02}:{:02},{:03}", h, m, s, frac)
    }

    fn to_srt(&self) -> String {
        format!(
            "{}\n{} --> {}\n{}\n",
            self.index,
            Self::format_timestamp(self.start_ms),
            Self::format_timestamp(self.end_ms),
            self.text
        )
    }
}

/// Parse an SRT file into entries.
fn parse_srt(content: &str) -> Result<Vec<SrtEntry>> {
    let mut entries = Vec::new();
    // Split on blank lines (each subtitle block is separated by one or more blank lines)
    let _blocks: Vec<&str> = content.split("\n\r\n").collect();
    // Also handle \n\n without \r
    let blocks: Vec<&str> = content
        .split("\n\n")
        .flat_map(|b| b.split("\r\n\r\n"))
        .collect();

    for block in blocks {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 3 {
            continue; // Need at least: index, timestamp, text
        }

        // Parse index
        let index: u32 = lines[0].trim().parse().unwrap_or(entries.len() as u32 + 1);

        // Parse timestamp line: "00:00:20,000 --> 00:00:24,400"
        let ts_line = lines[1].trim();
        let ts_parts: Vec<&str> = ts_line.split("-->").collect();
        if ts_parts.len() != 2 {
            continue;
        }
        let start_ms = parse_srt_timestamp(ts_parts[0].trim())?;
        let end_ms = parse_srt_timestamp(ts_parts[1].trim())?;

        // Text is everything from line 2 onward
        let text = lines[2..].join("\n");

        entries.push(SrtEntry {
            index,
            start_ms,
            end_ms,
            text,
        });
    }

    Ok(entries)
}

/// Parse an SRT timestamp like "00:01:23,456" into milliseconds.
fn parse_srt_timestamp(s: &str) -> Result<u64> {
    // Handle both comma and period as decimal separator
    let s = s.trim();
    let s = s.replace('.', ",");

    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        bail!("Invalid SRT timestamp: '{}'", s);
    }
    let h: u64 = parts[0].parse().context("Invalid hours")?;
    let m: u64 = parts[1].parse().context("Invalid minutes")?;
    let sec_parts: Vec<&str> = parts[2].split(',').collect();
    if sec_parts.len() != 2 {
        bail!("Invalid SRT timestamp seconds: '{}'", parts[2]);
    }
    let sec: u64 = sec_parts[0].parse().context("Invalid seconds")?;
    let ms: u64 = sec_parts[1].parse().context("Invalid milliseconds")?;

    Ok(h * 3_600_000 + m * 60_000 + sec * 1000 + ms)
}

// ---------------------------------------------------------------------------
// Helper: read the full MKV segment (EBML header + segment children)
// Returns (ebml, segment) after parsing.
// ---------------------------------------------------------------------------
struct MkvFullData {
    ebml: Ebml,
    info: Info,
    tracks: Option<Tracks>,
    clusters: Vec<Cluster>,
    tags: Vec<Tags>,
    attachments: Option<Attachments>,
    chapters: Option<Chapters>,
}

fn read_full_mkv(input: &PathBuf) -> Result<MkvFullData> {
    let mut reader = BufReader::new(File::open(input)?);
    let ebml = Ebml::read_from(&mut reader)?;

    let segment_header = Header::read_from(&mut reader)?;
    if segment_header.id != Segment::ID {
        bail!("Expected Segment element, got {}", segment_header.id);
    }

    let segment_data_start = reader.stream_position()?;

    let segment_size = if segment_header.size.is_unknown {
        u64::MAX
    } else {
        *segment_header.size
    };
    let segment_end = if segment_size == u64::MAX {
        u64::MAX
    } else {
        segment_data_start + segment_size
    };

    let mut info: Option<Info> = None;
    let mut tracks: Option<Tracks> = None;
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut tags: Vec<Tags> = Vec::new();
    let mut attachments: Option<Attachments> = None;
    let mut chapters: Option<Chapters> = None;

    loop {
        let pos = reader.stream_position()?;
        if pos >= segment_end {
            break;
        }
        let Ok(child_header) = Header::read_from(&mut reader) else {
            break;
        };

        match child_header.id {
            Tracks::ID => {
                tracks = Some(Tracks::read_element(&child_header, &mut reader)?);
            }
            Cluster::ID => {
                clusters.push(Cluster::read_element(&child_header, &mut reader)?);
            }
            Tags::ID => {
                tags.push(Tags::read_element(&child_header, &mut reader)?);
            }
            Info::ID => {
                info = Some(Info::read_element(&child_header, &mut reader)?);
            }
            Attachments::ID => {
                attachments = Some(Attachments::read_element(&child_header, &mut reader)?);
            }
            Chapters::ID => {
                chapters = Some(Chapters::read_element(&child_header, &mut reader)?);
            }
            _ => {
                // Skip unknown / SeekHead / Cues
                let size = *child_header.size as usize;
                let mut discard = vec![0u8; 8192.min(size)];
                let mut remaining = size;
                while remaining > 0 {
                    let to_read = remaining.min(discard.len());
                    reader.read_exact(&mut discard[..to_read])?;
                    remaining -= to_read;
                }
            }
        }
    }

    let info = info.context("No Info element found in segment")?;

    Ok(MkvFullData {
        ebml,
        info,
        tracks,
        clusters,
        tags,
        attachments,
        chapters,
    })
}

/// Write a complete MKV from the parsed data.
fn write_mkv(output: &PathBuf, data: &MkvFullData) -> Result<()> {
    let out_file = File::create(output)
        .with_context(|| format!("Failed to create output file {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);

    data.ebml.write_to(&mut writer)?;

    let segment = Segment {
        crc32: None,
        void: None,
        seek_head: vec![],
        info: data.info.clone(),
        cluster: data.clusters.clone(),
        tracks: data.tracks.clone(),
        cues: None,
        attachments: data.attachments.clone(),
        chapters: data.chapters.clone(),
        tags: data.tags.clone(),
    };

    segment.write_to(&mut writer)?;
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
#[derive(Parser)]
#[command(
    name = "mkv-strip",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("BUILD_DATE"), ")\n\nCreated by Digital Futures Consultancy LLP (Singapore) - https://DigitalFutures.Asia"),
    about = "Strip, extract, and add tracks in MKV files",
    after_help = "\nCreated by Digital Futures Consultancy LLP (Singapore) - https://DigitalFutures.Asia"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Shorthand: list tracks when no subcommand given, or when used with -l
    /// If provided without a subcommand, lists tracks in the file
    #[arg(short = 'l', long = "list")]
    list_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all tracks in an MKV file
    List {
        /// Path to the MKV file
        input: PathBuf,
    },
    /// Strip tracks and produce a new MKV file
    Strip {
        /// Input MKV file
        #[arg(short, long)]
        input: PathBuf,
        /// Output MKV file
        #[arg(short, long)]
        output: PathBuf,
        /// Track IDs to KEEP (comma-separated, e.g. "1,2,4").
        /// Keeps only the specified track numbers; all others are stripped.
        /// Use 'mkv-strip list' to see track IDs.
        #[arg(short = 'k', long = "keep", value_delimiter = ',')]
        keep_ids: Vec<u64>,
        /// Audio languages to KEEP (comma-separated, e.g. "eng,jpn"). Can be repeated.
        #[arg(short = 'a', long = "keep-audio", value_delimiter = ',')]
        keep_audio: Vec<String>,
        /// Audio languages to REMOVE (comma-separated, e.g. "fre,spa"). Can be repeated.
        #[arg(short = 'r', long = "remove-audio", value_delimiter = ',')]
        remove_audio: Vec<String>,
        /// Subtitle languages to KEEP (comma-separated). Can be repeated.
        #[arg(long = "keep-subtitle", value_delimiter = ',')]
        keep_subtitle: Vec<String>,
        /// Subtitle languages to REMOVE (comma-separated). Can be repeated.
        #[arg(long = "remove-subtitle", value_delimiter = ',')]
        remove_subtitle: Vec<String>,
        /// Remove ALL audio tracks
        #[arg(long)]
        no_audio: bool,
        /// Remove ALL subtitle tracks
        #[arg(long)]
        no_subtitle: bool,
        /// Remove ALL video tracks (dangerous!)
        #[arg(long)]
        no_video: bool,
        /// Set tracks as default by ID (comma-separated)
        #[arg(long = "set-default", value_delimiter = ',')]
        set_default: Vec<u64>,
        /// Clear default flag from tracks by ID (comma-separated)
        #[arg(long = "clear-default", value_delimiter = ',')]
        clear_default: Vec<u64>,
        /// Set tracks as forced by ID (comma-separated)
        #[arg(long = "set-forced", value_delimiter = ',')]
        set_forced: Vec<u64>,
        /// Clear forced flag from tracks by ID (comma-separated)
        #[arg(long = "clear-forced", value_delimiter = ',')]
        clear_forced: Vec<u64>,
        /// Set tracks as enabled by ID (comma-separated)
        #[arg(long = "set-enabled", value_delimiter = ',')]
        set_enabled: Vec<u64>,
        /// Clear enabled flag from tracks by ID (comma-separated)
        #[arg(long = "clear-enabled", value_delimiter = ',')]
        clear_enabled: Vec<u64>,
    },
    /// Extract subtitle tracks from an MKV file to SRT
    Extract {
        /// Input MKV file
        #[arg(short, long)]
        input: PathBuf,
        /// Output directory for SRT files (default: same as input)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
        /// Subtitle track numbers to extract (comma-separated). Default: all subtitle tracks.
        #[arg(short = 't', long = "tracks", value_delimiter = ',')]
        track_numbers: Vec<u64>,
        /// Subtitle languages to extract (comma-separated). Default: all.
        #[arg(short = 'l', long = "lang", value_delimiter = ',')]
        languages: Vec<String>,
    },
    /// Add an SRT subtitle file to an MKV file
    Add {
        /// Input MKV file
        #[arg(short, long)]
        input: PathBuf,
        /// SRT subtitle file to add
        #[arg(short, long)]
        srt: PathBuf,
        /// Output MKV file (default: overwrite input)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Language code for the subtitle track (e.g. "eng", "spa")
        #[arg(short, long, default_value = "und")]
        lang: String,
        /// BCP-47 language code (e.g. "en", "es-419")
        #[arg(long)]
        lang_bcp47: Option<String>,
        /// Track name (e.g. "English (SDH)")
        #[arg(short, long)]
        name: Option<String>,
        /// Set as default subtitle track
        #[arg(long)]
        default: bool,
        /// Set as forced subtitle track
        #[arg(long)]
        forced: bool,
        /// Set as hearing-impaired track (for users with hearing impairments)
        #[arg(long)]
        hearing_impaired: bool,
        /// Set as visual-impaired track (for users with visual impairments)
        #[arg(long)]
        visual_impaired: bool,
        /// Set as text descriptions track (describes video content for visually impaired users)
        #[arg(long)]
        descriptions: bool,
        /// Set as original language track
        #[arg(long)]
        original: bool,
        /// Set as commentary track
        #[arg(long)]
        commentary: bool,
    },

}

// ---------------------------------------------------------------------------
// Apply track flag modifications
// ---------------------------------------------------------------------------
fn apply_flag_mods(tracks: &mut Tracks, set_default: &[u64], clear_default: &[u64],
                   set_forced: &[u64], clear_forced: &[u64],
                   set_enabled: &[u64], clear_enabled: &[u64]) {
    let set_default_set: HashSet<u64> = set_default.iter().copied().collect();
    let clear_default_set: HashSet<u64> = clear_default.iter().copied().collect();
    let set_forced_set: HashSet<u64> = set_forced.iter().copied().collect();
    let clear_forced_set: HashSet<u64> = clear_forced.iter().copied().collect();
    let set_enabled_set: HashSet<u64> = set_enabled.iter().copied().collect();
    let clear_enabled_set: HashSet<u64> = clear_enabled.iter().copied().collect();

    for te in &mut tracks.track_entry {
        let tn = *te.track_number;
        if set_default_set.contains(&tn) { te.flag_default = FlagDefault(1); }
        if clear_default_set.contains(&tn) { te.flag_default = FlagDefault(0); }
        if set_forced_set.contains(&tn) { te.flag_forced = FlagForced(1); }
        if clear_forced_set.contains(&tn) { te.flag_forced = FlagForced(0); }
        if set_enabled_set.contains(&tn) { te.flag_enabled = FlagEnabled(1); }
        if clear_enabled_set.contains(&tn) { te.flag_enabled = FlagEnabled(0); }
    }
}

// ---------------------------------------------------------------------------
// List command
// ---------------------------------------------------------------------------
fn cmd_list(input: &PathBuf) -> Result<()> {
    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)
        .with_context(|| format!("Failed to parse MKV metadata from {}", input.display()))?;

    for (si, seg) in view.segments.iter().enumerate() {
        if view.segments.len() > 1 {
            println!("Segment {}", si + 1);
        }
        if let Some(ref tracks) = seg.tracks {
            let infos: Vec<TrackInfo> =
                tracks.track_entry.iter().map(TrackInfo::from_track_entry).collect();
            let table = TrackTable::build(&infos);
            println!("{}", table.header_line());
            println!("{}", table.separator_line());
            for row in &table.rows {
                println!("{}", table.row_line(row));
            }
        } else {
            println!("  (no tracks found)");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Strip command
// ---------------------------------------------------------------------------

fn block_matches_kept_track(data: &[u8], kept_tracks: &HashSet<u64>) -> bool {
    match track_number_from_block(data) {
        Some(tn) => kept_tracks.contains(&tn),
        None => true,
    }
}

fn cmd_strip(
    input: &PathBuf,
    output: &PathBuf,
    keep_ids: &[u64],
    keep_audio: &[String],
    remove_audio: &[String],
    keep_subtitle: &[String],
    remove_subtitle: &[String],
    no_audio: bool,
    no_subtitle: bool,
    no_video: bool,
    set_default: &[u64],
    clear_default: &[u64],
    set_forced: &[u64],
    clear_forced: &[u64],
    set_enabled: &[u64],
    clear_enabled: &[u64],
) -> Result<()> {
    // When --keep is used, delegate to the keep-by-ID logic
    if !keep_ids.is_empty() {
        return cmd_keep(input, output, keep_ids, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled);
    }

    if no_audio && !keep_audio.is_empty() {
        bail!("Cannot use --no-audio with --keep-audio");
    }
    if no_audio && !remove_audio.is_empty() {
        bail!("Cannot use --no-audio with --remove-audio");
    }
    if no_subtitle && !keep_subtitle.is_empty() {
        bail!("Cannot use --no-subtitle with --keep-subtitle");
    }
    if no_subtitle && !remove_subtitle.is_empty() {
        bail!("Cannot use --no-subtitle with --remove-subtitle");
    }

    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)
        .with_context(|| format!("Failed to parse MKV metadata from {}", input.display()))?;

    if view.segments.len() != 1 {
        bail!(
            "Expected exactly 1 segment, found {}. Multi-segment files are not yet supported.",
            view.segments.len()
        );
    }

    let seg_view = &view.segments[0];
    let tracks = seg_view
        .tracks
        .as_ref()
        .context("No Tracks element found in MKV file")?;

    let keep_audio_langs: Vec<String> = keep_audio.iter().map(|s| s.to_ascii_lowercase()).collect();
    let remove_audio_langs: Vec<String> = remove_audio.iter().map(|s| s.to_ascii_lowercase()).collect();
    let keep_sub_langs: Vec<String> = keep_subtitle.iter().map(|s| s.to_ascii_lowercase()).collect();
    let remove_sub_langs: Vec<String> = remove_subtitle.iter().map(|s| s.to_ascii_lowercase()).collect();

    let mut kept_track_numbers: HashSet<u64> = HashSet::new();
    let mut kept_infos: Vec<TrackInfo> = Vec::new();
    let mut removed_infos: Vec<TrackInfo> = Vec::new();

    for te in tracks.track_entry.iter() {
        let info = TrackInfo::from_track_entry(te);
        let lang_lower = info.language.to_ascii_lowercase();
        let lang_bcp_lower = info.language_bcp47.as_deref().map(|l| l.to_ascii_lowercase());

        let should_keep = match info.track_type {
            TRACK_TYPE_VIDEO => !no_video,
            TRACK_TYPE_AUDIO => {
                if no_audio {
                    false
                } else if !keep_audio_langs.is_empty() {
                    keep_audio_langs.iter().any(|k| {
                        lang_lower == *k || lang_bcp_lower.as_deref() == Some(k.as_str())
                    })
                } else if !remove_audio_langs.is_empty() {
                    !remove_audio_langs.iter().any(|r| {
                        lang_lower == *r || lang_bcp_lower.as_deref() == Some(r.as_str())
                    })
                } else {
                    true
                }
            }
            TRACK_TYPE_SUBTITLE => {
                if no_subtitle {
                    false
                } else if !keep_sub_langs.is_empty() {
                    keep_sub_langs.iter().any(|k| {
                        lang_lower == *k || lang_bcp_lower.as_deref() == Some(k.as_str())
                    })
                } else if !remove_sub_langs.is_empty() {
                    !remove_sub_langs.iter().any(|r| {
                        lang_lower == *r || lang_bcp_lower.as_deref() == Some(r.as_str())
                    })
                } else {
                    true
                }
            }
            _ => true,
        };

        if should_keep {
            kept_track_numbers.insert(info.number);
            kept_infos.push(info);
        } else {
            removed_infos.push(info);
        }
    }

    if kept_track_numbers.is_empty() {
        bail!("All tracks would be removed — refusing to write an empty MKV file.");
    }

    let all_infos: Vec<TrackInfo> = kept_infos.iter().chain(removed_infos.iter()).cloned().collect();
    let table = TrackTable::build(&all_infos);

    let label_w = 5;
    println!("  {}{}", pad_right("", label_w), table.header_line().trim_start());
    println!("  {}{}", pad_right("", label_w), table.separator_line().trim_start());

    for info in &kept_infos {
        let idx = all_infos.iter().position(|a| a.number == info.number).unwrap();
        let row = &table.rows[idx];
        println!("  {}{}", pad_right("KEEP", label_w), table.row_line(row).trim_start());
    }
    for info in &removed_infos {
        let idx = all_infos.iter().position(|a| a.number == info.number).unwrap();
        let row = &table.rows[idx];
        println!("  {}{}", pad_right("STRIP", label_w), table.row_line(row).trim_start());
    }

    // Phase 3: Re-read and reconstruct
    let mut full_reader = BufReader::new(File::open(input)?);
    let ebml = Ebml::read_from(&mut full_reader)?;
    let segment_header = Header::read_from(&mut full_reader)?;
    if segment_header.id != Segment::ID {
        bail!("Expected Segment element, got {}", segment_header.id);
    }
    let segment_data_start = full_reader.stream_position()?;

    let removed_track_uids: HashSet<u64> = tracks
        .track_entry
        .iter()
        .filter(|te| !kept_track_numbers.contains(&(*te.track_number).into()))
        .map(|te| *te.track_uid)
        .collect();

    full_reader.seek(std::io::SeekFrom::Start(segment_data_start))?;

    let mut filtered_tracks: Option<Tracks> = None;
    let mut filtered_clusters: Vec<Cluster> = Vec::new();
    let mut filtered_tags: Vec<Tags> = Vec::new();
    let mut info: Option<Info> = None;
    let mut attachments: Option<Attachments> = None;
    let mut chapters: Option<Chapters> = None;

    let segment_size = if segment_header.size.is_unknown { u64::MAX } else { *segment_header.size };
    let segment_end = if segment_size == u64::MAX { u64::MAX } else { segment_data_start + segment_size };

    loop {
        let pos = full_reader.stream_position()?;
        if pos >= segment_end { break; }
        let Ok(child_header) = Header::read_from(&mut full_reader) else { break; };

        match child_header.id {
            Tracks::ID => {
                let mut tracks_data = Tracks::read_element(&child_header, &mut full_reader)?;
                tracks_data.track_entry.retain(|te| kept_track_numbers.contains(&(*te.track_number).into()));
                apply_flag_mods(&mut tracks_data, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled);
                filtered_tracks = Some(tracks_data);
            }
            Cluster::ID => {
                let mut cluster = Cluster::read_element(&child_header, &mut full_reader)?;
                cluster.blocks.retain(|block| match block {
                    ClusterBlock::Simple(sb) => block_matches_kept_track(sb, &kept_track_numbers),
                    ClusterBlock::Group(bg) => block_matches_kept_track(&bg.block, &kept_track_numbers),
                });
                if !cluster.blocks.is_empty() {
                    filtered_clusters.push(cluster);
                }
            }
            Tags::ID => {
                let mut tags = Tags::read_element(&child_header, &mut full_reader)?;
                for tag in &mut tags.tag {
                    tag.targets.tag_track_uid.retain(|uid| !removed_track_uids.contains(&**uid));
                }
                filtered_tags.push(tags);
            }
            Info::ID => { info = Some(Info::read_element(&child_header, &mut full_reader)?); }
            Attachments::ID => { attachments = Some(Attachments::read_element(&child_header, &mut full_reader)?); }
            Chapters::ID => { chapters = Some(Chapters::read_element(&child_header, &mut full_reader)?); }
            _ => {
                let size = *child_header.size as usize;
                let mut discard = vec![0u8; 8192.min(size)];
                let mut remaining = size;
                while remaining > 0 {
                    let to_read = remaining.min(discard.len());
                    full_reader.read_exact(&mut discard[..to_read])?;
                    remaining -= to_read;
                }
            }
        }
    }

    let info = info.context("No Info element found in segment")?;
    let out_file = File::create(output).with_context(|| format!("Failed to create output file {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);
    ebml.write_to(&mut writer)?;

    let segment = Segment {
        crc32: None, void: None, seek_head: vec![], info,
        cluster: filtered_clusters, tracks: filtered_tracks, cues: None,
        attachments, chapters, tags: filtered_tags,
    };
    segment.write_to(&mut writer)?;
    writer.flush()?;

    println!();
    let n_removed = removed_infos.len();
    let n_kept = kept_infos.len();
    if n_removed == 0 {
        println!("No tracks removed.");
    } else {
        println!("✓ Kept {} track(s), stripped {} track(s)", n_kept, n_removed);
        let removed_table = TrackTable::build(&removed_infos);
        for row in &removed_table.rows {
            println!("  {}", removed_table.row_line(row).trim_start());
        }
    }
    println!("Output: {}", output.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Extract command
// ---------------------------------------------------------------------------

fn cmd_extract(
    input: &PathBuf,
    output_dir: &Option<PathBuf>,
    track_numbers: &[u64],
    languages: &[String],
) -> Result<()> {
    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)
        .with_context(|| format!("Failed to parse MKV metadata from {}", input.display()))?;

    if view.segments.len() != 1 {
        bail!("Multi-segment files are not yet supported.");
    }

    let seg_view = &view.segments[0];
    let mkv_tracks = seg_view.tracks.as_ref().context("No Tracks element found")?;

    // Find subtitle tracks
    let lang_filters: Vec<String> = languages.iter().map(|s| s.to_ascii_lowercase()).collect();
    let target_tracks: Vec<TrackInfo> = mkv_tracks.track_entry.iter()
        .filter_map(|te| {
            let info = TrackInfo::from_track_entry(te);
            if info.track_type != TRACK_TYPE_SUBTITLE {
                return None;
            }
            // Check if it's a text-based codec we can extract
            if !info.codec_id.starts_with("S_TEXT/") {
                return None; // skip image-based subtitles (VobSub, etc.)
            }
            // Filter by track number if specified
            if !track_numbers.is_empty() && !track_numbers.contains(&info.number) {
                return None;
            }
            // Filter by language if specified
            if !lang_filters.is_empty() {
                let lang_lower = info.language.to_ascii_lowercase();
                let lang_bcp_lower = info.language_bcp47.as_deref().map(|l| l.to_ascii_lowercase());
                let matches_lang = lang_filters.iter().any(|f| {
                    lang_lower == *f || lang_bcp_lower.as_deref() == Some(f.as_str())
                });
                if !matches_lang {
                    return None;
                }
            }
            Some(info)
        })
        .collect();

    if target_tracks.is_empty() {
        println!("No matching subtitle tracks found.");
        return Ok(());
    }

    // Determine output directory
    let out_dir = match output_dir {
        Some(d) => d.clone(),
        None => input.parent().unwrap_or(std::path::Path::new(".")).to_path_buf(),
    };
    std::fs::create_dir_all(&out_dir)?;

    let base_name = input.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let timestamp_scale: u64 = *seg_view.info.timestamp_scale;

    // Now read the full MKV to get cluster data
    let mut full_reader = BufReader::new(File::open(input)?);
    let _ebml_header = Header::read_from(&mut full_reader)?; // skip EBML
    let segment_header = Header::read_from(&mut full_reader)?;
    let segment_data_start = full_reader.stream_position()?;
    let segment_size = if segment_header.size.is_unknown { u64::MAX } else { *segment_header.size };
    let segment_end = if segment_size == u64::MAX { u64::MAX } else { segment_data_start + segment_size };

    // Collect subtitle frames per track
    let mut track_frames: std::collections::HashMap<u64, Vec<(u64, Option<u64>, Vec<u8>)>> =
        std::collections::HashMap::new();
    for t in &target_tracks {
        track_frames.insert(t.number, Vec::new());
    }

    loop {
        let pos = full_reader.stream_position()?;
        if pos >= segment_end { break; }
        let Ok(child_header) = Header::read_from(&mut full_reader) else { break; };

        match child_header.id {
            Cluster::ID => {
                let cluster = Cluster::read_element(&child_header, &mut full_reader)?;
                let cluster_ts: u64 = *cluster.timestamp;
                for frame_result in cluster.frames() {
                    let frame = match frame_result {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    if let Some(frames_vec) = track_frames.get_mut(&frame.track_number) {
                        // Convert timestamp to milliseconds
                        let ts_ms = (cluster_ts as i64 + frame.timestamp) as u64
                            * timestamp_scale / 1_000_000;
                        let duration_ms = frame.duration.map(|d| d.get() * timestamp_scale / 1_000_000);

                        // Collect frame data
                        let data_slices: Vec<&[u8]> = match &frame.data {
                            mkv_element::FrameData::Single(d) => vec![d],
                            mkv_element::FrameData::Multiple(v) => v.clone(),
                        };

                        for data in data_slices {
                            let _text = String::from_utf8_lossy(data).into_owned();
                            // For SRT, each "frame" becomes one entry
                            frames_vec.push((ts_ms, duration_ms, data.to_vec()));
                        }
                    }
                }
            }
            _ => {
                let size = *child_header.size as usize;
                let mut discard = vec![0u8; 8192.min(size)];
                let mut remaining = size;
                while remaining > 0 {
                    let to_read = remaining.min(discard.len());
                    full_reader.read_exact(&mut discard[..to_read])?;
                    remaining -= to_read;
                }
            }
        }
    }

    // Write SRT files
    for track in &target_tracks {
        let frames = track_frames.get(&track.number).unwrap();
        if frames.is_empty() {
            println!("  Track {} — no frames, skipping", track.number);
            continue;
        }

        let lang_suffix = if track.language != "und" { &track.language } else { "" };
        let name_suffix = track.name.as_deref().map(|n| format!(".{}", n.replace(' ', "_"))).unwrap_or_default();
        let srt_filename = format!("{}.{}.{}{}.srt", base_name, track.number, lang_suffix, name_suffix);
        let srt_path = out_dir.join(&srt_filename);

        let mut srt_content = String::new();
        for (i, (ts_ms, duration_ms, data)) in frames.iter().enumerate() {
            let start = *ts_ms;
            let end = *ts_ms + duration_ms.unwrap_or(2000); // default 2s if no duration
            let text = String::from_utf8_lossy(data);

            let entry = SrtEntry {
                index: (i + 1) as u32,
                start_ms: start,
                end_ms: end,
                text: text.to_string(),
            };
            srt_content.push_str(&entry.to_srt());
        }

        let mut f = File::create(&srt_path)?;
        f.write_all(srt_content.as_bytes())?;

        println!("  Track {} ({}): {} — {} frame(s)",
            track.number, track.language, srt_path.display(), frames.len());
    }

    println!();
    println!("✓ Extracted {} subtitle track(s) to {}", target_tracks.len(), out_dir.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Add command — inject an SRT file as a new subtitle track
// ---------------------------------------------------------------------------

fn cmd_add(
    input: &PathBuf,
    srt_path: &PathBuf,
    output: &Option<PathBuf>,
    lang: &str,
    lang_bcp47: &Option<String>,
    name: &Option<String>,
    default: bool,
    forced: bool,
    hearing_impaired: bool,
    visual_impaired: bool,
    descriptions: bool,
    original: bool,
    commentary: bool,
) -> Result<()> {
    // Parse the SRT file
    let srt_content = std::fs::read_to_string(srt_path)
        .with_context(|| format!("Failed to read SRT file {}", srt_path.display()))?;
    let srt_entries = parse_srt(&srt_content)
        .with_context(|| format!("Failed to parse SRT file {}", srt_path.display()))?;

    if srt_entries.is_empty() {
        bail!("SRT file contains no entries.");
    }

    println!("Loaded {} subtitle(s) from {}", srt_entries.len(), srt_path.display());

    // Read the full MKV
    let mut mkv_data = read_full_mkv(input)?;

    // Determine the next track number
    let max_track_num = mkv_data.tracks
        .as_ref()
        .map(|t| t.track_entry.iter().map(|te| *te.track_number).max().unwrap_or(0))
        .unwrap_or(0);
    let new_track_number = max_track_num + 1;

    // Generate a unique TrackUID
    let new_track_uid = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;

    // Get timestamp scale from Info
    let timestamp_scale: u64 = *mkv_data.info.timestamp_scale;

    // Convert SRT entries to MKV Cluster blocks
    // Group frames into clusters by time proximity (every ~5 seconds or on gap)
    let mut clusters_to_add: Vec<Cluster> = Vec::new();
    let mut current_blocks: Vec<ClusterBlock> = Vec::new();
    let mut current_cluster_ts: u64 = 0;

    for entry in &srt_entries {
        // Convert ms to segment ticks
        let start_ticks = entry.start_ms * 1_000_000 / timestamp_scale;

        // Start a new cluster if the timestamp gap is large or this is the first entry
        if current_blocks.is_empty() {
            current_cluster_ts = start_ticks;
        } else if (start_ticks as i64 - current_cluster_ts as i64).unsigned_abs() > 30000 {
            // Gap > 30s of ticks, start new cluster
            if !current_blocks.is_empty() {
                clusters_to_add.push(Cluster {
                    crc32: None,
                    void: None,
                    timestamp: Timestamp(current_cluster_ts),
                    position: None,
                    prev_size: None,
                    blocks: std::mem::take(&mut current_blocks),
                });
            }
            current_cluster_ts = start_ticks;
        }

        // Build the SimpleBlock for this subtitle frame
        // SimpleBlock format: [track_number_vint] [relative_ts i16] [flags u8] [data]
        let text_bytes = entry.text.as_bytes();
        let relative_ts = (start_ticks as i64 - current_cluster_ts as i64) as i16;

        let mut block_data = Vec::new();
        // Encode track number as VInt
        encode_vint(new_track_number, &mut block_data);
        // Relative timestamp (i16 big-endian)
        block_data.extend_from_slice(&relative_ts.to_be_bytes());
        // Flags: keyframe bit (0x80) set for subtitle frames
        block_data.push(0x80);
        // Frame data
        block_data.extend_from_slice(text_bytes);

        current_blocks.push(ClusterBlock::Simple(SimpleBlock(Bytes::from(block_data))));
    }

    // Flush remaining blocks
    if !current_blocks.is_empty() {
        clusters_to_add.push(Cluster {
            crc32: None,
            void: None,
            timestamp: Timestamp(current_cluster_ts),
            position: None,
            prev_size: None,
            blocks: current_blocks,
        });
    }

    // Add the new track entry
    let new_track_entry = TrackEntry {
        crc32: None,
        void: None,
        track_number: TrackNumber(new_track_number),
        track_uid: TrackUid(new_track_uid),
        track_type: TrackType(TRACK_TYPE_SUBTITLE),
        flag_enabled: FlagEnabled(1),
        flag_default: FlagDefault(if default { 1 } else { 0 }),
        flag_forced: FlagForced(if forced { 1 } else { 0 }),
        flag_hearing_impaired: if hearing_impaired { Some(FlagHearingImpaired(1)) } else { None },
        flag_visual_impaired: if visual_impaired { Some(FlagVisualImpaired(1)) } else { None },
        flag_text_descriptions: if descriptions { Some(FlagTextDescriptions(1)) } else { None },
        flag_original: if original { Some(FlagOriginal(1)) } else { None },
        flag_commentary: if commentary { Some(FlagCommentary(1)) } else { None },
        flag_lacing: FlagLacing(0),
        default_duration: None,
        default_decoded_field_duration: None,
        max_block_addition_id: MaxBlockAdditionId(0),
        block_addition_mapping: vec![],
        name: name.as_ref().map(|n| Name(n.clone())),
        language: Language(lang.to_string()),
        language_bcp47: lang_bcp47.as_ref().map(|l| LanguageBcp47(l.clone())),
        codec_id: CodecId("S_TEXT/UTF8".to_string()),
        codec_private: None,
        codec_name: None,
        codec_delay: CodecDelay(0),
        seek_pre_roll: SeekPreRoll(0),
        track_translate: vec![],
        video: None,
        audio: None,
        track_operation: None,
        content_encodings: None,
    };

    // Ensure tracks exist and add the entry
    if mkv_data.tracks.is_none() {
        mkv_data.tracks = Some(Tracks {
            crc32: None,
            void: None,
            track_entry: vec![],
        });
    }
    mkv_data.tracks.as_mut().unwrap().track_entry.push(new_track_entry);

    // Append clusters
    mkv_data.clusters.extend(clusters_to_add);

    // Determine output path
    let output_path = output.clone().unwrap_or_else(|| input.clone());

    write_mkv(&output_path, &mkv_data)?;

    println!();
    println!(
        "✓ Added subtitle track #{} (lang: {}, codec: S_TEXT/UTF8) — {} entries",
        new_track_number, lang, srt_entries.len()
    );
    if let Some(ref n) = name {
        println!("  Name: {}", n);
    }
    if default { println!("  Default: yes"); }
    if forced { println!("  Forced: yes"); }
    if hearing_impaired { println!("  Hearing-impaired: yes"); }
    if visual_impaired { println!("  Visual-impaired: yes"); }
    if descriptions { println!("  Descriptions: yes"); }
    if original { println!("  Original: yes"); }
    if commentary { println!("  Commentary: yes"); }
    println!("Output: {}", output_path.display());

    Ok(())
}

/// Encode a u64 as an EBML VInt into a buffer.
fn encode_vint(value: u64, buf: &mut Vec<u8>) {
    if value < 0x80 {
        // 1-byte: 0x1xxxxxxx
        buf.push(0x80 | (value as u8));
    } else if value < 0x4000 {
        // 2-byte: 0x01xxxxxx xxxxxxxx
        buf.push(0x40 | ((value >> 8) as u8));
        buf.push(value as u8);
    } else if value < 0x200000 {
        // 3-byte
        buf.push(0x20 | ((value >> 16) as u8));
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value < 0x10000000 {
        // 4-byte
        buf.push(0x10 | ((value >> 24) as u8));
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        // For larger values, use more bytes (rare for track numbers)
        let _encoded = VInt64::new(value).as_encoded();
        let size = VInt64::encode_size(value);
        let mut sbuf = [0u8; 8];
        let slice = &mut sbuf[8 - size..];
        slice.copy_from_slice(&value.to_be_bytes()[8 - size..]);
        slice[0] |= 1u8 << (8 - size);
        buf.extend_from_slice(slice);
    }
}


// ---------------------------------------------------------------------------
// Keep command — keep only specified track IDs, strip the rest
// ---------------------------------------------------------------------------

fn cmd_keep(input: &PathBuf, output: &PathBuf, keep_ids: &[u64],
           set_default: &[u64], clear_default: &[u64],
           set_forced: &[u64], clear_forced: &[u64],
           set_enabled: &[u64], clear_enabled: &[u64]) -> Result<()> {
    if keep_ids.is_empty() {
        bail!("No track IDs specified. Use -k or --keep with comma-separated track numbers (e.g. 1,2,4)");
    }

    let kept_set: HashSet<u64> = keep_ids.iter().copied().collect();

    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)
        .with_context(|| format!("Failed to parse MKV metadata from {}", input.display()))?;

    if view.segments.len() != 1 {
        bail!(
            "Expected exactly 1 segment, found {}. Multi-segment files are not yet supported.",
            view.segments.len()
        );
    }

    let seg_view = &view.segments[0];
    let tracks = seg_view
        .tracks
        .as_ref()
        .context("No Tracks element found in MKV file")?;

    let mut kept_infos: Vec<TrackInfo> = Vec::new();
    let mut removed_infos: Vec<TrackInfo> = Vec::new();
    let mut kept_track_numbers: HashSet<u64> = HashSet::new();

    for te in tracks.track_entry.iter() {
        let info = TrackInfo::from_track_entry(te);
        if kept_set.contains(&info.number) {
            kept_infos.push(info.clone());
            kept_track_numbers.insert(info.number);
        } else {
            removed_infos.push(info);
        }
    }

    // Validate that all requested IDs actually exist
    for id in keep_ids {
        if !kept_set.contains(id) || !tracks.track_entry.iter().any(|te| *te.track_number == *id) {
            bail!("Track ID {} not found in the MKV file. Use 'mkv-strip list' to see available tracks.", id);
        }
    }

    if kept_track_numbers.is_empty() {
        bail!("No valid track IDs to keep. Use 'mkv-strip list' to see available tracks.");
    }

    let all_infos: Vec<TrackInfo> = kept_infos.iter().chain(removed_infos.iter()).cloned().collect();
    let table = TrackTable::build(&all_infos);

    let label_w = 5;
    println!("  {}{}", pad_right("", label_w), table.header_line().trim_start());
    println!("  {}{}", pad_right("", label_w), table.separator_line().trim_start());

    for info in &kept_infos {
        let idx = all_infos.iter().position(|a| a.number == info.number).unwrap();
        let row = &table.rows[idx];
        println!("  {}{}", pad_right("KEEP", label_w), table.row_line(row).trim_start());
    }
    for info in &removed_infos {
        let idx = all_infos.iter().position(|a| a.number == info.number).unwrap();
        let row = &table.rows[idx];
        println!("  {}{}", pad_right("STRIP", label_w), table.row_line(row).trim_start());
    }

    // Re-read and reconstruct
    let mut full_reader = BufReader::new(File::open(input)?);
    let ebml = Ebml::read_from(&mut full_reader)?;
    let segment_header = Header::read_from(&mut full_reader)?;
    if segment_header.id != Segment::ID {
        bail!("Expected Segment element, got {}", segment_header.id);
    }
    let segment_data_start = full_reader.stream_position()?;

    let removed_track_uids: HashSet<u64> = tracks
        .track_entry
        .iter()
        .filter(|te| !kept_track_numbers.contains(&(*te.track_number).into()))
        .map(|te| *te.track_uid)
        .collect();

    full_reader.seek(std::io::SeekFrom::Start(segment_data_start))?;

    let mut filtered_tracks: Option<Tracks> = None;
    let mut filtered_clusters: Vec<Cluster> = Vec::new();
    let mut filtered_tags: Vec<Tags> = Vec::new();
    let mut info: Option<Info> = None;
    let mut attachments: Option<Attachments> = None;
    let mut chapters: Option<Chapters> = None;

    let segment_size = if segment_header.size.is_unknown { u64::MAX } else { *segment_header.size };
    let segment_end = if segment_size == u64::MAX { u64::MAX } else { segment_data_start + segment_size };

    loop {
        let pos = full_reader.stream_position()?;
        if pos >= segment_end { break; }
        let Ok(child_header) = Header::read_from(&mut full_reader) else { break; };

        match child_header.id {
            Tracks::ID => {
                let mut tracks_data = Tracks::read_element(&child_header, &mut full_reader)?;
                tracks_data.track_entry.retain(|te| kept_track_numbers.contains(&(*te.track_number).into()));
                apply_flag_mods(&mut tracks_data, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled);
                filtered_tracks = Some(tracks_data);
            }
            Cluster::ID => {
                let mut cluster = Cluster::read_element(&child_header, &mut full_reader)?;
                cluster.blocks.retain(|block| match block {
                    ClusterBlock::Simple(sb) => block_matches_kept_track(sb, &kept_track_numbers),
                    ClusterBlock::Group(bg) => block_matches_kept_track(&bg.block, &kept_track_numbers),
                });
                if !cluster.blocks.is_empty() {
                    filtered_clusters.push(cluster);
                }
            }
            Tags::ID => {
                let mut tags = Tags::read_element(&child_header, &mut full_reader)?;
                for tag in &mut tags.tag {
                    tag.targets.tag_track_uid.retain(|uid| !removed_track_uids.contains(&**uid));
                }
                filtered_tags.push(tags);
            }
            Info::ID => { info = Some(Info::read_element(&child_header, &mut full_reader)?); }
            Attachments::ID => { attachments = Some(Attachments::read_element(&child_header, &mut full_reader)?); }
            Chapters::ID => { chapters = Some(Chapters::read_element(&child_header, &mut full_reader)?); }
            _ => {
                let size = *child_header.size as usize;
                let mut discard = vec![0u8; 8192.min(size)];
                let mut remaining = size;
                while remaining > 0 {
                    let to_read = remaining.min(discard.len());
                    full_reader.read_exact(&mut discard[..to_read])?;
                    remaining -= to_read;
                }
            }
        }
    }

    let info = info.context("No Info element found in segment")?;
    let out_file = File::create(output).with_context(|| format!("Failed to create output file {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);
    ebml.write_to(&mut writer)?;

    let segment = Segment {
        crc32: None, void: None, seek_head: vec![], info,
        cluster: filtered_clusters, tracks: filtered_tracks, cues: None,
        attachments, chapters, tags: filtered_tags,
    };
    segment.write_to(&mut writer)?;
    writer.flush()?;

    println!();
    let n_removed = removed_infos.len();
    let n_kept = kept_infos.len();
    if n_removed == 0 {
        println!("No tracks removed.");
    } else {
        println!("✓ Kept {} track(s), stripped {} track(s)", n_kept, n_removed);
        let removed_table = TrackTable::build(&removed_infos);
        for row in &removed_table.rows {
            println!("  {}", removed_table.row_line(row).trim_start());
        }
    }
    println!("Output: {}", output.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
fn main() -> Result<()> {
    let cli = Cli::parse();

    // -l/--list: shorthand for 'list' command
    if let Some(ref list_file) = cli.list_file {
        return cmd_list(list_file);
    }

    // Default: no subcommand → show help
    match cli.command {
        Some(Commands::List { input }) => cmd_list(&input),
        Some(Commands::Strip {
            input, output, keep_ids, keep_audio, remove_audio,
            keep_subtitle, remove_subtitle,
            no_audio, no_subtitle, no_video,
            set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled,
        }) => cmd_strip(&input, &output, &keep_ids, &keep_audio, &remove_audio,
            &keep_subtitle, &remove_subtitle, no_audio, no_subtitle, no_video,
            &set_default, &clear_default, &set_forced, &clear_forced, &set_enabled, &clear_enabled),
        Some(Commands::Extract { input, output_dir, track_numbers, languages }) =>
            cmd_extract(&input, &output_dir, &track_numbers, &languages),
        Some(Commands::Add { input, srt, output, lang, lang_bcp47, name, default, forced, hearing_impaired, visual_impaired, descriptions, original, commentary }) =>
            cmd_add(&input, &srt, &output, &lang, &lang_bcp47, &name, default, forced, hearing_impaired, visual_impaired, descriptions, original, commentary),
        None => {
            let mut cmd = Cli::command();
            cmd.print_help()?;
            Ok(())
        }
    }
}