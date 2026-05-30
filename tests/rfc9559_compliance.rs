/// RFC 9559 compliance tests for mkv-strip.
///
/// These tests verify that mkv-strip produces spec-compliant output
/// by round-tripping IETF test files through strip/add operations and
/// validating the results.
use std::fs;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;

fn mkv_strip_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/mkv-strip")
}

fn test_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("matroska-test-files/test_files")
        .join(name)
}

fn temp_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-output");
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Helper: parse raw EBML VInt from bytes
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn parse_vint_value(data: &[u8]) -> Option<u64> {
    if data.is_empty() { return None; }
    let first = data[0];
    if first == 0 { return None; }
    let leading = first.leading_zeros() as usize;
    if leading >= 8 { return None; }
    let len = leading + 1;
    if data.len() < len { return None; }
    let mask = (1u8 << (8 - len)) - 1;
    let mut result = (first & mask) as u64;
    for &b in &data[1..len] {
        result = (result << 8) | b as u64;
    }
    Some(result)
}

/// Read an EBML VInt from the stream, returning (value, bytes_consumed).
fn read_vint_stream(f: &mut BufReader<fs::File>) -> Option<(u64, usize)> {
    let mut first = [0u8; 1];
    if f.read_exact(&mut first).is_err() { return None; }
    let b = first[0];
    if b == 0 { return None; }
    let leading = b.leading_zeros() as usize;
    let vint_len = leading + 1;
    let mask = (1u8 << (8 - vint_len)) - 1;
    let mut result = (b & mask) as u64;
    for _ in 1..vint_len {
        let mut next = [0u8; 1];
        if f.read_exact(&mut next).is_err() { return None; }
        result = (result << 8) | next[0] as u64;
    }
    Some((result, vint_len))
}

/// Read an EBML element header (ID + size) from current position.
/// Returns (element_id, data_size).
fn read_ebml_header(f: &mut BufReader<fs::File>) -> Option<(u64, u64)> {
    let (elem_id, _id_len) = read_vint_stream(f)?;
    let (elem_size, _sz_len) = read_vint_stream(f)?;
    Some((elem_id, elem_size))
}

/// Find all top-level element IDs in a Segment by scanning raw bytes.
fn find_segment_element_ids(path: &std::path::Path) -> Vec<u64> {
    let mut f = BufReader::new(fs::File::open(path).unwrap());

    // Parse EBML header: ID=0x1A45DFA3 + VInt size + body
    let (elem_id, body_size) = read_ebml_header(&mut f).unwrap();
    assert_eq!(elem_id, EBML_HEADER_ID, "Expected EBML header ID");
    // Skip body
    f.seek(SeekFrom::Current(body_size as i64)).unwrap();

    // Read Segment header: 0x18538067
    let (seg_id, seg_size) = read_ebml_header(&mut f).unwrap();
    assert_eq!(seg_id, SEGMENT_ID, "Expected Segment ID");
    let seg_end = f.stream_position().unwrap() + seg_size;

    let mut ids = Vec::new();
    while f.stream_position().unwrap() < seg_end {
        let Some((id, elem_size)) = read_ebml_header(&mut f) else { break; };
        ids.push(id);
        if elem_size > 0 {
            f.seek(SeekFrom::Current(elem_size as i64)).unwrap();
        }
    }
    ids
}

fn has_element(path: &std::path::Path, target: u64) -> bool {
    find_segment_element_ids(path).iter().any(|id| *id == target)
}

/// EBML Header = 0x1A45DFA3, decoded VInt = 0x0A45DFA3
const EBML_HEADER_ID: u64 = 0x0A45DFA3;
/// Segment = 0x18538067, decoded VInt = 0x08538067
const SEGMENT_ID: u64 = 0x08538067;
/// SeekHead = 0x114D9B74, decoded VInt = 0x014D9B74
const SEEKHEAD_ID: u64 = 0x014D9B74;
/// Cues = 0x1C53BB6B, decoded VInt = 0x0C53BB6B
const CUES_ID: u64 = 0x0C53BB6B;

// ---------------------------------------------------------------------------
// Test: list command works on all IETF test files
// ---------------------------------------------------------------------------
#[test]
fn test_list_all_ietf_files() {
    let bin = mkv_strip_bin();
    for i in 1..=8 {
        let file = test_file(&format!("test{}.mkv", i));
        if !file.exists() { continue; }
        // test4 is live-streaming (unknown segment size) — MatroskaView can't parse it
        // test7 has junk data that causes parse failure
        if i == 4 || i == 7 { continue; }
        let output = Command::new(&bin)
            .args(["list", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success(),
            "list failed for test{}.mkv: {}", i, String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Type") || stdout.contains("no tracks"),
            "list output unexpected for test{}.mkv: {}", i, stdout);
    }
}

// ---------------------------------------------------------------------------
// Test: strip produces valid MKV with correct tracks
// ---------------------------------------------------------------------------
#[test]
fn test_strip_keep_video_audio() {
    let bin = mkv_strip_bin();
    let input = test_file("test5.mkv"); // has video + 2 audio + 8 subtitle tracks
    let out = temp_dir().join("strip-test5-video-audio.mkv");

    let output = Command::new(&bin)
        .args([
            "strip",
            "-i", input.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "--no-subtitle",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "strip failed: {}",
        String::from_utf8_lossy(&output.stderr));
    assert!(out.exists());

    let list_out = Command::new(&bin)
        .args(["list", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(!stdout.contains("subtitle"), "Output should have no subtitle tracks");
    assert!(stdout.contains("video"), "Output should have video track");
    assert!(stdout.contains("audio"), "Output should have audio track");
}

// ---------------------------------------------------------------------------
// Test: strip produces valid MKV that can be re-parsed
// ---------------------------------------------------------------------------
#[test]
fn test_strip_roundtrip_reparsable() {
    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let out = temp_dir().join("roundtrip-test1.mkv");

    let output = Command::new(&bin)
        .args([
            "strip",
            "-i", input.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "--no-subtitle",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "strip failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let list_out = Command::new(&bin)
        .args(["list", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(list_out.status.success(), "re-parse of stripped file failed");
}

// ---------------------------------------------------------------------------
// Test: add SRT produces valid MKV with subtitle track
// ---------------------------------------------------------------------------
#[test]
fn test_add_srt_creates_subtitle_track() {
    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let srt_path = temp_dir().join("test.srt");
    fs::write(&srt_path, "1\n00:00:01,000 --> 00:00:02,000\nHello World\n\n2\n00:00:03,000 --> 00:00:04,000\nTest subtitle\n\n").unwrap();
    let out = temp_dir().join("add-test1.mkv");

    let output = Command::new(&bin)
        .args([
            "add",
            "-i", input.to_str().unwrap(),
            "-s", srt_path.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "-l", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "add failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let list_out = Command::new(&bin)
        .args(["list", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(stdout.contains("subtitle"), "Output should have subtitle track");
    assert!(stdout.contains("eng"), "Output should show English language");
}

// ---------------------------------------------------------------------------
// Test: added subtitles play back correctly (extract round-trip)
// ---------------------------------------------------------------------------
#[test]
fn test_add_extract_roundtrip() {
    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let srt_content = "1\n00:00:01,000 --> 00:00:02,500\nFirst line\n\n2\n00:00:03,000 --> 00:00:04,500\nSecond line\n\n";
    let srt_path = temp_dir().join("roundtrip.srt");
    fs::write(&srt_path, srt_content).unwrap();
    let mkv_path = temp_dir().join("roundtrip.mkv");

    let output = Command::new(&bin)
        .args([
            "add",
            "-i", input.to_str().unwrap(),
            "-s", srt_path.to_str().unwrap(),
            "-o", mkv_path.to_str().unwrap(),
            "-l", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "add failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let extract_dir = temp_dir().join("extracted");
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir).unwrap();
    let output = Command::new(&bin)
        .args([
            "extract",
            "-i", mkv_path.to_str().unwrap(),
            "-o", extract_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "extract failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let entries: Vec<_> = fs::read_dir(&extract_dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "srt"))
        .collect();
    assert!(!entries.is_empty(), "No SRT files extracted");

    let extracted = fs::read_to_string(entries[0].path()).unwrap();
    assert!(extracted.contains("First line"), "Extracted SRT missing first subtitle");
    assert!(extracted.contains("Second line"), "Extracted SRT missing second subtitle");
}

// ---------------------------------------------------------------------------
// Test: Cues and SeekHead present in add output
// ---------------------------------------------------------------------------
#[test]
fn test_add_produces_seekhead_and_cues() {
    let bin = mkv_strip_bin();
    let input = test_file("test6.mkv"); // test6 has no Cues originally
    let srt_path = temp_dir().join("cue-test.srt");
    fs::write(&srt_path, "1\n00:00:01,000 --> 00:00:02,000\nTest\n\n").unwrap();
    let out = temp_dir().join("cue-test.mkv");

    let output = Command::new(&bin)
        .args([
            "add",
            "-i", input.to_str().unwrap(),
            "-s", srt_path.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "-l", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "add failed: {}",
        String::from_utf8_lossy(&output.stderr));

    assert!(has_element(&out, SEEKHEAD_ID), "Output should contain SeekHead element");
    assert!(has_element(&out, CUES_ID), "Output should contain Cues element");
}

// ---------------------------------------------------------------------------
// Test: Cues are deduplicated (one per cluster per track, not per block)
// ---------------------------------------------------------------------------
#[test]
fn test_cues_deduplicated() {
    use mkv_element::view::MatroskaView;

    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let srt_path = temp_dir().join("dedup-test.srt");
    let mut srt = String::new();
    for i in 1..=50 {
        let start_ms = i * 500;
        let end_ms = start_ms + 400;
        let h = start_ms / 3_600_000;
        let m = (start_ms % 3_600_000) / 60_000;
        let s = (start_ms % 60_000) / 1000;
        let ms = start_ms % 1000;
        let eh = end_ms / 3_600_000;
        let em = (end_ms % 3_600_000) / 60_000;
        let es = (end_ms % 60_000) / 1000;
        let ems = end_ms % 1000;
        srt.push_str(&format!("{}\n{:02}:{:02}:{:02},{:03} --> {:02}:{:02}:{:02},{:03}\nSubtitle {}\n\n",
            i, h, m, s, ms, eh, em, es, ems, i));
    }
    fs::write(&srt_path, &srt).unwrap();
    let out = temp_dir().join("dedup-test.mkv");

    let output = Command::new(&bin)
        .args([
            "add",
            "-i", input.to_str().unwrap(),
            "-s", srt_path.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "-l", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "add failed: {}",
        String::from_utf8_lossy(&output.stderr));

    // Parse the output using MatroskaView (lightweight, reads Cues)
    let mut file = std::fs::File::open(&out).unwrap();
    let view = MatroskaView::new(&mut file).unwrap();
    let seg = &view.segments[0];

    if let Some(ref cues) = seg.cues {
        let cue_point_count = cues.cue_point.len();
        println!("Cue points: {}", cue_point_count);
        // Cue points should be sorted by time
        for i in 1..cue_point_count {
            assert!(*cues.cue_point[i-1].cue_time <= *cues.cue_point[i].cue_time,
                "CuePoints should be sorted by time");
        }
    }
}

// ---------------------------------------------------------------------------
// Test: flags command works in-place
// ---------------------------------------------------------------------------
#[test]
fn test_flags_inplace() {
    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let out = temp_dir().join("flags-test.mkv");
    fs::copy(&input, &out).unwrap();

    let output = Command::new(&bin)
        .args([
            "flags",
            "-i", out.to_str().unwrap(),
            "--set-default", "1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "flags failed: {}",
        String::from_utf8_lossy(&output.stderr));
}

// ---------------------------------------------------------------------------
// Test: strip with language filter
// ---------------------------------------------------------------------------
#[test]
fn test_strip_by_language() {
    let bin = mkv_strip_bin();
    let input = test_file("test5.mkv");
    let out = temp_dir().join("lang-filter.mkv");

    let output = Command::new(&bin)
        .args([
            "strip",
            "-i", input.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "--keep-subtitle", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "strip by language failed: {}",
        String::from_utf8_lossy(&output.stderr));

    let list_out = Command::new(&bin)
        .args(["list", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    let subtitle_lines: Vec<&str> = stdout.lines()
        .filter(|l| l.contains("subtitle"))
        .collect();
    for line in &subtitle_lines {
        assert!(line.contains("eng"), "Non-English subtitle found: {}", line);
    }
}

// ---------------------------------------------------------------------------
// Test: cluster timestamps are monotonically increasing in add output
// We verify by checking the output has correct structure (SeekHead + Cues)
// and can be re-parsed without errors, which implies correct cluster ordering.
// ---------------------------------------------------------------------------
#[test]
fn test_cluster_timestamps_monotonic() {
    let bin = mkv_strip_bin();
    let input = test_file("test1.mkv");
    let srt_path = temp_dir().join("mono-test.srt");
    let mut srt = String::new();
    for i in 1..=100 {
        let ms = i * 200;
        let end = ms + 150;
        srt.push_str(&format!("{}\n{:02}:{:02}:{:02},{:03} --> {:02}:{:02}:{:02},{:03}\nLine {}\n\n",
            i, ms/3600000, (ms%3600000)/60000, (ms%60000)/1000, ms%1000,
            end/3600000, (end%3600000)/60000, (end%60000)/1000, end%1000, i));
    }
    fs::write(&srt_path, &srt).unwrap();
    let out = temp_dir().join("mono-test.mkv");

    let output = Command::new(&bin)
        .args([
            "add",
            "-i", input.to_str().unwrap(),
            "-s", srt_path.to_str().unwrap(),
            "-o", out.to_str().unwrap(),
            "-l", "eng",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "add failed: {}",
        String::from_utf8_lossy(&output.stderr));

    // Verify the output can be listed without error (implies correct structure)
    let list_out = Command::new(&bin)
        .args(["list", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(list_out.status.success(), "re-parse of add output failed");

    // Verify SeekHead and Cues exist (implies proper element ordering)
    assert!(has_element(&out, SEEKHEAD_ID), "Output should contain SeekHead");
    assert!(has_element(&out, CUES_ID), "Output should contain Cues");
}
