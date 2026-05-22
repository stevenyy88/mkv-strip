use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, CommandFactory};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use bytes::Bytes;
use mkv_element::io::blocking_impl::{ReadElement, ReadFrom, WriteTo};
use mkv_element::prelude::*;
use mkv_element::view::{MatroskaView, SegmentView};
use mkv_element::ClusterBlock;

// ---------------------------------------------------------------------------
// Track type constants (from Matroska spec)
// ---------------------------------------------------------------------------
const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 17;

// EBML Element IDs for track flags (decoded VInt64 values, matching *Type::ID)
// These are the VInt values with the marker bit stripped,
// matching how mkv-element stores them internally.
const EBML_ID_TRACK_ENTRY: u64 = 0x2E;       // TrackEntry: encoded 0xAE, decoded 0x2E
const EBML_ID_FLAG_ENABLED: u64 = 0x39;       // FlagEnabled: encoded 0xB9, decoded 0x39
const EBML_ID_FLAG_DEFAULT: u64 = 0x08;       // FlagDefault: encoded 0x88, decoded 0x08
const EBML_ID_FLAG_FORCED: u64 = 0x15AA;      // FlagForced: encoded 0x55AA, decoded 0x15AA
const EBML_ID_FLAG_HEARING_IMPAIRED: u64 = 0x15AB; // FlagHearingImpaired
const EBML_ID_FLAG_VISUAL_IMPAIRED: u64 = 0x15AC;  // FlagVisualImpaired
const EBML_ID_FLAG_TEXT_DESCRIPTIONS: u64 = 0x15AD; // FlagTextDescriptions
const EBML_ID_FLAG_ORIGINAL: u64 = 0x15AE;    // FlagOriginal
const EBML_ID_FLAG_COMMENTARY: u64 = 0x15AF;  // FlagCommentary
const EBML_ID_TRACK_NUMBER: u64 = 0x57;       // TrackNumber: encoded 0xD7, decoded 0x57

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
    // Strip the marker bit: the marker is the first '1' bit from MSB.
    // For a vint_len-byte VInt, the marker is bit (8 - vint_len) of the first byte.
    // Mask to keep only the value bits.
    let marker_mask = (1u8 << (8 - vint_len)) - 1; // e.g. vint_len=4 → 0x0F
    let mut result: u64 = (first & marker_mask) as u64;
    for &b in &data[1..vint_len] {
        result = (result << 8) | b as u64;
    }
    Some(result)
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
// Streaming I/O helpers for memory-efficient processing
// ---------------------------------------------------------------------------

/// Encode a u64 value as an EBML VInt with the minimum number of bytes.
fn encode_vint_size(value: u64) -> Vec<u8> {
    if value <= 0x7F {
        vec![(0x80 | (value & 0x7F)) as u8]
    } else if value <= 0x3FFF {
        vec![0x40 | ((value >> 8) & 0x3F) as u8, (value & 0xFF) as u8]
    } else if value <= 0x1FFFFF {
        vec![0x20 | ((value >> 16) & 0x1F) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    } else if value <= 0x0FFFFFFF {
        vec![0x10 | ((value >> 24) & 0x0F) as u8, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    } else if value <= 0x07FFFFFFFF {
        vec![0x08 | ((value >> 32) & 0x07) as u8, ((value >> 24) & 0xFF) as u8, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    } else if value <= 0x03FFFFFFFFFF {
        vec![0x04 | ((value >> 40) & 0x03) as u8, ((value >> 32) & 0xFF) as u8, ((value >> 24) & 0xFF) as u8, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    } else if value <= 0x01FFFFFFFFFFFF {
        vec![0x02 | ((value >> 48) & 0x01) as u8, ((value >> 40) & 0xFF) as u8, ((value >> 32) & 0xFF) as u8, ((value >> 24) & 0xFF) as u8, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    } else {
        vec![0x01, ((value >> 48) & 0xFF) as u8, ((value >> 40) & 0xFF) as u8, ((value >> 32) & 0xFF) as u8, ((value >> 24) & 0xFF) as u8, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8]
    }
}

/// Raw-copy exactly `n` bytes from reader to writer using a 64KB buffer.
/// This avoids allocating a buffer proportional to `n`.
fn copy_n_bytes<R: Read + ?Sized, W: Write + ?Sized>(r: &mut R, w: &mut W, mut n: u64) -> Result<()> {
    let mut buf = [0u8; 65536];
    while n > 0 {
        let to_read = n.min(buf.len() as u64) as usize;
        r.read_exact(&mut buf[..to_read])?;
        w.write_all(&mut buf[..to_read])?;
        n -= to_read as u64;
    }
    Ok(())
}

/// Write an EBML Void element to fill `gap` bytes.
fn write_void_fill<W: Write + ?Sized>(w: &mut W, gap: usize) -> Result<()> {
    if gap == 0 { return Ok(()); }
    if gap == 2 {
        w.write_all(&[0xEC, 0x80])?;
    } else if gap > 2 {
        // Void element: 0xEC (1 byte) + size VInt + zero padding
        // We need: 1 + size_vint_len + content_len = gap
        // Start with content_len = gap - 2 (assuming 1-byte size VInt)
        // If content_len > 127, we need 2-byte size VInt, so content_len = gap - 3
        let (size_vint, content_len) = if gap - 2 <= 127 {
            (encode_vint_size((gap - 2) as u64), gap - 2)
        } else {
            (encode_vint_size((gap - 3) as u64), gap - 3)
        };
        w.write_all(&[0xEC])?;
        w.write_all(&size_vint)?;
        for _ in 0..content_len {
            w.write_all(&[0x00])?;
        }
    }
    // gap == 1 is impossible to fill with valid EBML; just write a zero byte
    Ok(())
}

/// Stream an MKV file with track filtering, using constant memory regardless of file size.
/// Phase 1: Read metadata (tracks, tags, etc.) via MatroskaView — lightweight, no clusters.
/// Phase 2: For each cluster in the input:
///   - If ALL tracks are kept: raw-copy the entire cluster (no decode/encode needed)
///   - If SOME tracks are removed: parse cluster children, write only kept blocks
/// Phase 3: Write Tags, patch Segment size.
fn stream_mkv_with_filter<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    seg_view: &SegmentView,
    kept_track_numbers: &HashSet<u64>,
    filtered_tracks: &Option<Tracks>,
    filtered_tags: &[Tags],
    filtered_attachments: &Option<Attachments>,
    filtered_chapters: &Option<Chapters>,
) -> Result<u64> {
    let all_tracks_kept = seg_view.tracks.as_ref()
        .map(|t| t.track_entry.iter().all(|te| kept_track_numbers.contains(&(*te.track_number).into())))
        .unwrap_or(true);

    // Read EBML header
    let ebml = Ebml::read_from(reader)?;
    let segment_header = Header::read_from(reader)?;
    if segment_header.id != Segment::ID {
        bail!("Expected Segment element, got {}", segment_header.id);
    }
    let segment_data_start = reader.stream_position()?;

    // Write EBML header + Segment header with placeholder size
    ebml.write_to(writer)?;
    writer.write_all(&[0x18, 0x53, 0x80, 0x67])?; // Segment ID
    let size_offset = writer.stream_position()?;
    writer.write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?; // 8-byte placeholder
    let content_start = writer.stream_position()?;

    // Write metadata
    seg_view.info.write_to(writer)?;
    if let Some(ref tracks) = filtered_tracks {
        tracks.write_to(writer)?;
    }
    if let Some(ref attachments) = filtered_attachments {
        attachments.write_to(writer)?;
    }
    if let Some(ref chapters) = filtered_chapters {
        chapters.write_to(writer)?;
    }

    // Stream clusters
    let segment_size = if segment_header.size.is_unknown { u64::MAX } else { *segment_header.size };
    let segment_end = if segment_size == u64::MAX { u64::MAX } else { segment_data_start + segment_size };

    reader.seek(SeekFrom::Start(segment_data_start))?;
    let mut cluster_count: u64 = 0;

    loop {
        let pos = reader.stream_position()?;
        if pos >= segment_end { break; }
        let Ok(child_header) = Header::read_from(reader) else { break; };
        let header_len = reader.stream_position()? - pos;

        match child_header.id {
            Cluster::ID => {
                if all_tracks_kept {
                    // ALL tracks kept — raw copy the entire cluster (header + body)
                    // No decode/encode needed — zero extra memory
                    reader.seek(SeekFrom::Start(pos))?;
                    copy_n_bytes(reader, writer, header_len + *child_header.size)?;
                    cluster_count += 1;
                } else {
                    // Some tracks removed — need to filter blocks within the cluster
                    // Parse the cluster body to find block elements
                    let _cluster_body_start = reader.stream_position()?;
                    let _cluster_body_size = *child_header.size;

                    // Write the cluster header (ID + size) first
                    // We need to re-encode the cluster with the filtered body size,
                    // but we don't know the filtered size yet.
                    // Strategy: buffer the filtered cluster body, measure its size, then write.
                    // For large clusters with only a few blocks removed, this is suboptimal,
                    // but it's correct and memory use is bounded by the filtered body size.
                    //
                    // Better strategy: write cluster header with unknown size,
                    // stream filtered children, then go back and patch.
                    // But that requires another seek-back which complicates things.
                    //
                    // Simplest correct approach: use Cluster::read_element for filtered clusters,
                    // but this loads the entire cluster body. However, individual clusters in
                    // real MKV files are typically < 32MB, so this is acceptable.
                    // The key improvement is: when ALL tracks are kept (the common case),
                    // we raw-copy with zero memory.
                    let mut cluster = Cluster::read_element(&child_header, reader)?;
                    let _original_blocks = cluster.blocks.len();
                    cluster.blocks.retain(|block| match block {
                        ClusterBlock::Simple(sb) => block_matches_kept_track(sb, kept_track_numbers),
                        ClusterBlock::Group(bg) => block_matches_kept_track(&bg.block, kept_track_numbers),
                    });
                    if !cluster.blocks.is_empty() {
                        cluster.write_to(writer)?;
                        cluster_count += 1;
                    }
                }
            }
            _ => {
                // Non-cluster element (Tags, Cues, etc.) — skip it
                // We already have metadata from MatroskaView and write our own Tags at the end
                copy_n_bytes(reader, &mut std::io::sink(), *child_header.size)?;
            }
        }
    }

    // Write Tags
    for tag in filtered_tags.iter() {
        tag.write_to(writer)?;
    }

    // Patch Segment size
    let content_end = writer.stream_position()?;
    let content_size = content_end - content_start;
    let size_bytes = encode_vint_size(content_size);
    writer.seek(SeekFrom::Start(size_offset))?;
    writer.write_all(&size_bytes)?;
    let gap = 8 - size_bytes.len();
    write_void_fill(writer, gap)?;

    writer.seek(SeekFrom::End(0))?;
    writer.flush()?;

    Ok(cluster_count)
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
    /// Modify track flags in an MKV file in-place (no re-encode)
    Flags {
        /// Input MKV file (modified in-place)
        #[arg(short, long)]
        input: PathBuf,
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
// Flags command — modify track flags in-place
// ---------------------------------------------------------------------------

/// A flag element found in the file with its byte position.
#[allow(dead_code)]
#[derive(Debug)]
struct FlagLocation {
    /// Byte offset of the flag element's ID in the file
    id_offset: u64,
    /// Byte offset of the flag element's data (after ID + size)
    data_offset: u64,
    /// Size of the data portion in bytes
    data_size: u64,
}

/// Scan a single EBML element header (ID + size) from the current position.
/// Returns (element_id, size_value, bytes_consumed).
fn read_element_header_at(reader: &mut (impl Read + Seek), pos: u64) -> Result<(u64, u64, u64)> {
    reader.seek(std::io::SeekFrom::Start(pos))?;
    let mut id_buf = [0u8; 8];
    reader.read_exact(&mut id_buf[..1])?;
    let leading = id_buf[0].leading_zeros() as usize;
    let id_len = if leading >= 8 { bail!("Invalid EBML ID at offset {}", pos); } else { leading + 1 };
    if id_len > 1 {
        reader.read_exact(&mut id_buf[1..id_len])?;
    }
    let element_id = parse_vint_value(&id_buf[..id_len])
        .context("Failed to parse EBML element ID")?;

    let size_start = pos + id_len as u64;
    reader.seek(std::io::SeekFrom::Start(size_start))?;
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf[..1])?;
    let size_leading = size_buf[0].leading_zeros() as usize;
    let size_len = if size_leading >= 8 {
        // Unknown size (all 1s)
        return Ok((element_id, u64::MAX, (id_len + 1) as u64));
    } else {
        size_leading + 1
    };
    if size_len > 1 {
        reader.seek(std::io::SeekFrom::Start(size_start))?;
        reader.read_exact(&mut size_buf[..size_len])?;
    }
    let raw_size = parse_vint_value(&size_buf[..size_len]).unwrap_or(0);

    let header_len = (id_len + size_len) as u64;
    Ok((element_id, raw_size, header_len))
}

/// Find all flag elements within a TrackEntry that starts at `te_start` with `te_size` bytes.
/// Returns a map of EBML flag element ID -> FlagLocation.
fn find_flags_in_track_entry(
    reader: &mut (impl Read + Seek),
    te_start: u64,
    te_header_len: u64,
    te_size: u64,
) -> Result<(u64, std::collections::HashMap<u64, FlagLocation>)> {
    let te_data_start = te_start + te_header_len;
    let te_end = te_data_start + te_size;
    let mut track_number: u64 = 0;
    let mut flags = std::collections::HashMap::new();

    let mut pos = te_data_start;
    while pos < te_end {
        let (elem_id, elem_size, header_len) = read_element_header_at(reader, pos)?;
        let data_offset = pos + header_len;

        if elem_id == EBML_ID_TRACK_NUMBER {
            // Read the track number as a big-endian unsigned integer (NOT VInt)
            reader.seek(std::io::SeekFrom::Start(data_offset))?;
            let mut tn_buf = [0u8; 8];
            let read_len = elem_size.min(8) as usize;
            reader.read_exact(&mut tn_buf[..read_len])?;
            track_number = u64::from_be_bytes(tn_buf) >> (8 * (8 - read_len));
        } else if matches!(elem_id,
            EBML_ID_FLAG_ENABLED | EBML_ID_FLAG_DEFAULT | EBML_ID_FLAG_FORCED |
            EBML_ID_FLAG_HEARING_IMPAIRED | EBML_ID_FLAG_VISUAL_IMPAIRED |
            EBML_ID_FLAG_TEXT_DESCRIPTIONS | EBML_ID_FLAG_ORIGINAL | EBML_ID_FLAG_COMMENTARY
        ) {
            flags.insert(elem_id, FlagLocation {
                id_offset: pos,
                data_offset,
                data_size: elem_size,
            });
        }

        if elem_size == u64::MAX { break; }
        pos = data_offset + elem_size;
    }

    Ok((track_number, flags))
}

/// An in-place flag modification request.
struct FlagMod {
    flag_id: u64,
    value: u8,
}

fn cmd_flags(
    input: &PathBuf,
    set_default: &[u64],
    clear_default: &[u64],
    set_forced: &[u64],
    clear_forced: &[u64],
    set_enabled: &[u64],
    clear_enabled: &[u64],
) -> Result<()> {
    // Build a map of track_number -> list of flag modifications
    let mut mods: std::collections::HashMap<u64, Vec<FlagMod>> = std::collections::HashMap::new();

    for &id in set_default   { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_DEFAULT,   value: 1 }); }
    for &id in clear_default { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_DEFAULT,   value: 0 }); }
    for &id in set_forced   { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_FORCED,    value: 1 }); }
    for &id in clear_forced { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_FORCED,    value: 0 }); }
    for &id in set_enabled   { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_ENABLED,   value: 1 }); }
    for &id in clear_enabled { mods.entry(id).or_default().push(FlagMod { flag_id: EBML_ID_FLAG_ENABLED,   value: 0 }); }

    if mods.is_empty() {
        bail!("No flag modifications specified. Use --set-default, --clear-default, --set-forced, --clear-forced, --set-enabled, or --clear-enabled.");
    }

    // Validate track IDs exist using MatroskaView
    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)
        .with_context(|| format!("Failed to parse MKV metadata from {}", input.display()))?;
    if view.segments.len() != 1 {
        bail!("Multi-segment files are not yet supported.");
    }
    let tracks = view.segments[0].tracks.as_ref().context("No Tracks element found")?;
    let valid_ids: HashSet<u64> = tracks.track_entry.iter().map(|te| *te.track_number).collect();
    for &id in mods.keys() {
        if !valid_ids.contains(&id) {
            bail!("Track ID {} not found in the MKV file. Use 'mkv-strip list' to see available tracks.", id);
        }
    }
    drop(view);
    drop(reader);

    // Now scan the raw file to find TrackEntry elements and their flags
    // We use the full-read approach since we need to find exact byte positions
    let mut reader = BufReader::new(File::open(input)?);
    let file_size = reader.seek(std::io::SeekFrom::End(0))?;
    reader.seek(std::io::SeekFrom::Start(0))?;

    // Parse EBML header to find where Segment starts
    let _ebml = Ebml::read_from(&mut reader)?;
    let seg_header = Header::read_from(&mut reader)?;
    if seg_header.id != Segment::ID {
        bail!("Expected Segment element");
    }
    let seg_data_start = reader.stream_position()?;
    let seg_size = if *seg_header.size == u64::MAX { file_size - seg_data_start } else { *seg_header.size };
    let seg_end = seg_data_start + seg_size;

    // Scan for the Tracks element within the Segment
    let mut tracks_pos: Option<(u64, u64, u64)> = None; // (start, header_len, size)
    let mut pos = seg_data_start;
    while pos < seg_end {
        let (elem_id, elem_size, header_len) = read_element_header_at(&mut reader, pos)?;
        if elem_id == *Tracks::ID {
            tracks_pos = Some((pos, header_len, elem_size));
            break;
        }
        if elem_size == u64::MAX { break; }
        pos += header_len + elem_size;
    }
    let (tracks_start, tracks_header_len, tracks_size) = tracks_pos
        .context("No Tracks element found in MKV file")?;
    let tracks_data_start = tracks_start + tracks_header_len;
    let tracks_end = tracks_data_start + tracks_size;

    // Find TrackEntry elements and their flags
    let mut modifications: Vec<(u64, u8)> = Vec::new(); // (data_offset, new_value)
    let mut modified_tracks: HashSet<u64> = HashSet::new();
    let mut needs_insertion = false;

    let mut te_pos = tracks_data_start;
    while te_pos < tracks_end {
        let (te_id, te_size, te_header_len) = read_element_header_at(&mut reader, te_pos)?;
        if te_id != EBML_ID_TRACK_ENTRY {
            if te_size == u64::MAX { break; }
            te_pos += te_header_len + te_size;
            continue;
        }
        let te_data_start = te_pos + te_header_len;
        let te_end = te_data_start + te_size;

        let (track_number, found_flags) = find_flags_in_track_entry(&mut reader, te_pos, te_header_len, te_size)?;

        if let Some(track_mods) = mods.get(&track_number) {
            for fm in track_mods {
                if let Some(loc) = found_flags.get(&fm.flag_id) {
                    // Flag element exists — we can overwrite in-place
                    if loc.data_size != 1 {
                        // Safety check: flag data should be 1 byte (0 or 1)
                        // If it's larger, we can still write but need to be careful
                        // For safety, only in-place modify if size is 1
                        needs_insertion = true;
                        break;
                    }
                    modifications.push((loc.data_offset, fm.value));
                    modified_tracks.insert(track_number);
                } else {
                    // Flag element doesn't exist — need insertion (requires rewrite)
                    needs_insertion = true;
                }
            }
        }

        if te_size == u64::MAX { break; }
        te_pos = te_end;
    }

    if needs_insertion {
        // Some flags don't exist yet — fall back to full rewrite
        drop(reader);
        return cmd_flags_rewrite(input, mods);
    }

    // All flags exist and are 1-byte — do in-place modification
    drop(reader);
    let mut file = std::fs::OpenOptions::new().write(true).open(input)?;
    for (data_offset, new_value) in &modifications {
        file.seek(std::io::SeekFrom::Start(*data_offset))?;
        file.write_all(&[*new_value])?;
    }
    file.flush()?;
    drop(file);

    // Show the result
    let mut reader = BufReader::new(File::open(input)?);
    let view2 = MatroskaView::new(&mut reader)?;
    let tracks2 = view2.segments[0].tracks.as_ref().unwrap();
    let infos: Vec<TrackInfo> = tracks2.track_entry.iter()
        .map(|te| TrackInfo::from_track_entry(te))
        .collect();

    println!();
    let table = TrackTable::build(&infos);
    println!("  {}", table.header_line().trim_start());
    println!("  {}", table.separator_line().trim_start());
    for row in &table.rows {
        println!("  {}", table.row_line(row).trim_start());
    }
    println!();
    let mod_names: Vec<String> = modified_tracks.iter().map(|t| format!("#{}", t)).collect();
    println!("✓ Modified flags in-place for track(s): {}", mod_names.join(", "));

    Ok(())
}

/// Fallback: full rewrite when flag elements need to be inserted.
fn cmd_flags_rewrite(
    input: &PathBuf,
    mods: std::collections::HashMap<u64, Vec<FlagMod>>,
) -> Result<()> {
    let mut mkv_data = read_full_mkv(input)?;
    let tracks = mkv_data.tracks.as_mut().context("No Tracks element")?;

    let mut modified_tracks: HashSet<u64> = HashSet::new();
    for te in &mut tracks.track_entry {
        let tn = *te.track_number;
        if let Some(track_mods) = mods.get(&tn) {
            for fm in track_mods {
                match fm.flag_id {
                    EBML_ID_FLAG_DEFAULT   => te.flag_default = FlagDefault(fm.value as u64),
                    EBML_ID_FLAG_FORCED    => te.flag_forced = FlagForced(fm.value as u64),
                    EBML_ID_FLAG_ENABLED   => te.flag_enabled = FlagEnabled(fm.value as u64),
                    EBML_ID_FLAG_HEARING_IMPAIRED => {
                        te.flag_hearing_impaired = if fm.value == 1 { Some(FlagHearingImpaired(1)) } else { Some(FlagHearingImpaired(0)) };
                    }
                    EBML_ID_FLAG_VISUAL_IMPAIRED => {
                        te.flag_visual_impaired = if fm.value == 1 { Some(FlagVisualImpaired(1)) } else { Some(FlagVisualImpaired(0)) };
                    }
                    EBML_ID_FLAG_TEXT_DESCRIPTIONS => {
                        te.flag_text_descriptions = if fm.value == 1 { Some(FlagTextDescriptions(1)) } else { Some(FlagTextDescriptions(0)) };
                    }
                    EBML_ID_FLAG_ORIGINAL => {
                        te.flag_original = if fm.value == 1 { Some(FlagOriginal(1)) } else { Some(FlagOriginal(0)) };
                    }
                    EBML_ID_FLAG_COMMENTARY => {
                        te.flag_commentary = if fm.value == 1 { Some(FlagCommentary(1)) } else { Some(FlagCommentary(0)) };
                    }
                    _ => {}
                }
                modified_tracks.insert(tn);
            }
        }
    }

    // Write to a temp file, then replace original
    let tmp_path = input.with_extension("mkv-strip-tmp");
    write_mkv(&tmp_path, &mkv_data)?;
    std::fs::rename(&tmp_path, input)
        .with_context(|| format!("Failed to replace {} with modified file", input.display()))?;

    // Show the result
    let mut reader = BufReader::new(File::open(input)?);
    let view = MatroskaView::new(&mut reader)?;
    let tracks2 = view.segments[0].tracks.as_ref().unwrap();
    let infos: Vec<TrackInfo> = tracks2.track_entry.iter()
        .map(|te| TrackInfo::from_track_entry(te))
        .collect();

    println!();
    let table = TrackTable::build(&infos);
    println!("  {}", table.header_line().trim_start());
    println!("  {}", table.separator_line().trim_start());
    for row in &table.rows {
        println!("  {}", table.row_line(row).trim_start());
    }
    println!();
    let mod_names: Vec<String> = modified_tracks.iter().map(|t| format!("#{}", t)).collect();
    println!("✓ Modified flags for track(s): {}", mod_names.join(", "));
    println!("  (flags were rewritten — file structure updated)");

    Ok(())
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

    // Phase 1: Read metadata using MatroskaView (lightweight — no clusters loaded)
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

    // Phase 2: Prepare filtered metadata
    let removed_track_uids: HashSet<u64> = tracks
        .track_entry
        .iter()
        .filter(|te| !kept_track_numbers.contains(&(*te.track_number).into()))
        .map(|te| *te.track_uid)
        .collect();

    let filtered_tracks = {
        let mut t = seg_view.tracks.clone();
        if let Some(ref mut tracks_data) = t {
            tracks_data.track_entry.retain(|te| kept_track_numbers.contains(&(*te.track_number).into()));
            apply_flag_mods(tracks_data, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled);
        }
        t
    };
    let filtered_tags: Vec<Tags> = {
        let mut tags = seg_view.tags.clone();
        for tag in &mut tags {
            for t in &mut tag.tag {
                t.targets.tag_track_uid.retain(|uid| !removed_track_uids.contains(&**uid));
            }
        }
        tags
    };

    // Phase 3: Stream the MKV file with track filtering
    let out_file = File::create(output).with_context(|| format!("Failed to create output file {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);
    let mut reader = BufReader::new(File::open(input)?);

    let cluster_count = stream_mkv_with_filter(
        &mut reader, &mut writer,
        seg_view, &kept_track_numbers,
        &filtered_tracks, &filtered_tags,
        &seg_view.attachments, &seg_view.chapters,
    )?;

    println!();
    let n_removed = removed_infos.len();
    let n_kept = kept_infos.len();
    if n_removed == 0 {
        println!("No tracks removed.");
    } else {
        println!("✓ Kept {} track(s), stripped {} track(s) ({} clusters)", n_kept, n_removed, cluster_count);
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

    // Phase 1: Read metadata using MatroskaView (lightweight — no clusters loaded)
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

    // Phase 2: Prepare filtered metadata
    let removed_track_uids: HashSet<u64> = tracks
        .track_entry
        .iter()
        .filter(|te| !kept_track_numbers.contains(&(*te.track_number).into()))
        .map(|te| *te.track_uid)
        .collect();

    let filtered_tracks = {
        let mut t = seg_view.tracks.clone();
        if let Some(ref mut tracks_data) = t {
            tracks_data.track_entry.retain(|te| kept_track_numbers.contains(&(*te.track_number).into()));
            apply_flag_mods(tracks_data, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled);
        }
        t
    };
    let filtered_tags: Vec<Tags> = {
        let mut tags = seg_view.tags.clone();
        for tag in &mut tags {
            for t in &mut tag.tag {
                t.targets.tag_track_uid.retain(|uid| !removed_track_uids.contains(&**uid));
            }
        }
        tags
    };

    // Phase 3: Stream the MKV file with track filtering
    let out_file = File::create(output).with_context(|| format!("Failed to create output file {}", output.display()))?;
    let mut writer = BufWriter::new(out_file);
    let mut reader = BufReader::new(File::open(input)?);

    let cluster_count = stream_mkv_with_filter(
        &mut reader, &mut writer,
        seg_view, &kept_track_numbers,
        &filtered_tracks, &filtered_tags,
        &seg_view.attachments, &seg_view.chapters,
    )?;

    println!();
    let n_removed = removed_infos.len();
    let n_kept = kept_infos.len();
    if n_removed == 0 {
        println!("No tracks removed.");
    } else {
        println!("✓ Kept {} track(s), stripped {} track(s) ({} clusters)", n_kept, n_removed, cluster_count);
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
        Some(Commands::Flags { input, set_default, clear_default, set_forced, clear_forced, set_enabled, clear_enabled }) =>
            cmd_flags(&input, &set_default, &clear_default, &set_forced, &clear_forced, &set_enabled, &clear_enabled),
        Some(Commands::Add { input, srt, output, lang, lang_bcp47, name, default, forced, hearing_impaired, visual_impaired, descriptions, original, commentary }) =>
            cmd_add(&input, &srt, &output, &lang, &lang_bcp47, &name, default, forced, hearing_impaired, visual_impaired, descriptions, original, commentary),
        None => {
            let mut cmd = Cli::command();
            cmd.print_help()?;
            Ok(())
        }
    }
}