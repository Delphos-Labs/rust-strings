use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::io::{self, Read};

use rust_strings::{
    scan, Encoding, HitFinish, HitId, HitStart, ScanError, ScanOptions, ScanOptionsError,
    ScanSummary, SinkControl, StringSink,
};

#[derive(Debug, Clone, Eq, PartialEq)]
struct Record {
    start: HitStart,
    text: String,
    finish: HitFinish,
}

#[derive(Default)]
struct CollectSink {
    active: HashMap<HitId, (HitStart, String)>,
    seen: HashSet<HitId>,
    records: Vec<Record>,
    aborted: Vec<HitId>,
    max_active: usize,
}

impl StringSink for CollectSink {
    type Error = TestError;

    fn start(&mut self, id: HitId, start: HitStart) -> Result<SinkControl, Self::Error> {
        if !self.seen.insert(id) || self.active.insert(id, (start, String::new())).is_some() {
            return Err(TestError("duplicate hit id"));
        }
        self.max_active = self.max_active.max(self.active.len());
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, id: HitId, text: &str) -> Result<SinkControl, Self::Error> {
        self.active
            .get_mut(&id)
            .ok_or(TestError("chunk for inactive hit"))?
            .1
            .push_str(text);
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, id: HitId, finish: HitFinish) -> Result<(), Self::Error> {
        let (start, text) = self
            .active
            .remove(&id)
            .ok_or(TestError("finish for inactive hit"))?;
        self.records.push(Record {
            start,
            text,
            finish,
        });
        Ok(())
    }

    fn abort(&mut self, id: HitId) -> Result<(), Self::Error> {
        self.active
            .remove(&id)
            .ok_or(TestError("abort for inactive hit"))?;
        self.aborted.push(id);
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct TestError(&'static str);

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for TestError {}

fn options(min_length: usize, encodings: &[Encoding]) -> ScanOptions {
    ScanOptions::new(min_length, encodings.iter().copied()).unwrap()
}

fn utf16(text: &str, big_endian: bool, alignment: u8) -> Vec<u8> {
    let mut bytes = if alignment == 0 {
        Vec::new()
    } else {
        vec![0xff]
    };
    for unit in text.encode_utf16().chain([0]) {
        bytes.extend(if big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        });
    }
    bytes
}

fn append_utf16(bytes: &mut Vec<u8>, text: &str, big_endian: bool, terminate: bool) {
    for unit in text.encode_utf16().chain(terminate.then_some(0)) {
        bytes.extend(if big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        });
    }
}

#[track_caller]
fn matching_record<'a>(sink: &'a CollectSink, text: &str, offset: u64) -> &'a Record {
    sink.records
        .iter()
        .find(|record| record.text == text && record.start.offset.get() == offset)
        .unwrap_or_else(|| panic!("missing record {text:?} at {offset}: {:?}", sink.records))
}

/// Asserts that the record for `text` at `offset` from `encoding` exists and is shadowed.
#[track_caller]
fn assert_shadowed(sink: &CollectSink, text: &str, offset: u64, encoding: Encoding) {
    let record = sink
        .records
        .iter()
        .find(|record| {
            record.text == text
                && record.start.offset.get() == offset
                && record.start.encoding == encoding
        })
        .unwrap_or_else(|| {
            panic!(
                "missing {encoding} record {text:?} at {offset}: {:?}",
                sink.records
            )
        });
    assert!(record.finish.shadowed, "{record:?}");
}

/// The opposite-endian reading of ASCII text stored as UTF-16: each byte pair is swapped, so
/// every character `c` reads as U+`c`00.
fn swapped(text: &str) -> String {
    text.chars()
        .map(|character| char::from_u32(u32::from(character) << 8).unwrap())
        .collect()
}

#[test]
fn scan_options_validate_the_minimum_and_encodings() {
    assert_eq!(
        ScanOptions::new(0, [Encoding::ASCII]),
        Err(ScanOptionsError::ZeroMinimumLength)
    );
    assert_eq!(ScanOptions::new(3, []), Err(ScanOptionsError::NoEncodings));
    let options = ScanOptions::new(3, [Encoding::ASCII, Encoding::ASCII]).unwrap();
    assert_eq!(options.encodings(), &[Encoding::ASCII]);
}

#[test]
fn ascii_candidate_finishes_at_eof() {
    let mut reader = &b"before\0after"[..];
    let mut sink = CollectSink::default();
    let summary = scan(&mut reader, &options(4, &[Encoding::ASCII]), &mut sink).unwrap();

    let record = matching_record(&sink, "after", 7);
    assert_eq!(record.finish.source_length.get(), 5);
    assert_eq!(record.finish.character_count, 5);
    assert_eq!(summary.bytes_read, 12);
    assert_eq!(summary.candidate_count, 2);
}

#[test]
fn ascii_rejects_non_ascii_bytes() {
    let mut reader = &b"left\xffright"[..];
    let mut sink = CollectSink::default();
    scan(&mut reader, &options(4, &[Encoding::ASCII]), &mut sink).unwrap();

    matching_record(&sink, "left", 0);
    matching_record(&sink, "right", 5);
    assert_eq!(sink.records.len(), 2);
}

#[test]
fn line_breaks_end_candidates_and_tabs_stay_inside() {
    let mut reader = &b"echo one\r\necho two\n\tthree"[..];
    let mut sink = CollectSink::default();
    scan(&mut reader, &options(4, &[Encoding::ASCII]), &mut sink).unwrap();

    matching_record(&sink, "echo one", 0);
    matching_record(&sink, "echo two", 10);
    matching_record(&sink, "\tthree", 19);
    assert_eq!(sink.records.len(), 3, "{:?}", sink.records);

    for (encoding, big_endian) in [(Encoding::UTF16LE, false), (Encoding::UTF16BE, true)] {
        let bytes = utf16(
            "line one\r\nline two\u{2028}line\tsix\u{2029}line ten",
            big_endian,
            0,
        );
        let mut reader = bytes.as_slice();
        let mut sink = CollectSink::default();
        scan(&mut reader, &options(4, &[encoding]), &mut sink).unwrap();

        assert_eq!(
            matching_record(&sink, "line one", 0).start.encoding,
            encoding
        );
        // Offsets are bytes, two per UTF-16 code unit. "line one" and "\r\n" are ten units.
        assert_eq!(
            matching_record(&sink, "line two", 20).start.encoding,
            encoding
        );
        // "line two" and U+2028 are nine more; a tab stays inside "line\tsix".
        assert_eq!(
            matching_record(&sink, "line\tsix", 38).start.encoding,
            encoding
        );
        // "line\tsix" and U+2029 are nine more.
        assert_eq!(
            matching_record(&sink, "line ten", 56).start.encoding,
            encoding
        );
        assert!(
            sink.records
                .iter()
                .all(|record| !record.text.contains(['\n', '\r', '\u{2028}', '\u{2029}'])),
            "{:?}",
            sink.records
        );
    }
}

#[test]
fn utf16_supports_unicode_surrogates_endianness_and_both_alignments() {
    let text = "Aé😀中";
    for (encoding, big_endian) in [(Encoding::UTF16LE, false), (Encoding::UTF16BE, true)] {
        for alignment in 0..=1 {
            let bytes = utf16(text, big_endian, alignment);
            let mut reader = bytes.as_slice();
            let mut sink = CollectSink::default();
            scan(&mut reader, &options(4, &[encoding]), &mut sink).unwrap();

            let record = matching_record(&sink, text, u64::from(alignment));
            assert_eq!(record.start.encoding, encoding);
            assert_eq!(record.finish.character_count, 4);
            assert_eq!(record.finish.source_length.get(), 10);
            assert_eq!(record.finish.decoded_utf8_length, text.len() as u64);
        }
    }
}

struct ShortReader<'a> {
    bytes: &'a [u8],
    chunk_size: usize,
}

impl Read for ShortReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = self.bytes.len().min(self.chunk_size).min(buffer.len());
        buffer[..length].copy_from_slice(&self.bytes[..length]);
        self.bytes = &self.bytes[length..];
        Ok(length)
    }
}

#[test]
fn all_decoder_state_survives_short_reads() {
    let bytes = utf16("short 😀 reads", false, 1);
    for chunk_size in 1..=7 {
        let mut reader = ShortReader {
            bytes: &bytes,
            chunk_size,
        };
        let mut sink = CollectSink::default();
        scan(&mut reader, &options(5, &[Encoding::UTF16LE]), &mut sink).unwrap();
        matching_record(&sink, "short 😀 reads", 1);
    }
}

#[test]
fn invalid_surrogates_end_a_candidate_and_recover() {
    let units = [
        b'A' as u16,
        b'B' as u16,
        0xd800,
        b'C' as u16,
        b'D' as u16,
        0xdc00,
        b'E' as u16,
        b'F' as u16,
        0,
    ];
    let bytes = units
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(&mut reader, &options(2, &[Encoding::UTF16LE]), &mut sink).unwrap();

    matching_record(&sink, "AB", 0);
    matching_record(&sink, "CD", 6);
    matching_record(&sink, "EF", 12);
}

#[test]
fn stable_ids_disambiguate_overlapping_active_candidates() {
    let mut reader = &b"ABCDEFGH"[..];
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(2, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE]),
        &mut sink,
    )
    .unwrap();

    assert!(sink.max_active > 1);
    assert!(sink.active.is_empty());
    assert_eq!(sink.seen.len(), sink.records.len() + sink.aborted.len());
}

#[derive(Default)]
struct SkipSink {
    chunks: usize,
    target: Option<HitId>,
    finish: Option<HitFinish>,
}

impl StringSink for SkipSink {
    type Error = TestError;

    fn start(&mut self, id: HitId, start: HitStart) -> Result<SinkControl, Self::Error> {
        if start.offset.get() == 0 {
            self.target = Some(id);
        }
        Ok(SinkControl::SkipCurrent)
    }

    fn chunk(&mut self, _id: HitId, _text: &str) -> Result<SinkControl, Self::Error> {
        self.chunks += 1;
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, id: HitId, finish: HitFinish) -> Result<(), Self::Error> {
        if self.target == Some(id) {
            self.finish = Some(finish);
        }
        Ok(())
    }

    fn abort(&mut self, _id: HitId) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn skip_current_stops_text_but_preserves_finish_metadata() {
    let bytes = utf16("é😀", false, 0);
    let mut reader = bytes.as_slice();
    let mut sink = SkipSink::default();
    let summary = scan(&mut reader, &options(2, &[Encoding::UTF16LE]), &mut sink).unwrap();

    assert_eq!(sink.chunks, 0);
    assert_eq!(sink.finish.unwrap().decoded_utf8_length, 6);
    assert_eq!(summary.skipped_candidate_count, summary.candidate_count);
}

struct FailingReader {
    sent_data: bool,
}

impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.sent_data {
            return Err(io::Error::other("injected read failure"));
        }
        self.sent_data = true;
        buffer[..4].copy_from_slice(b"open");
        Ok(4)
    }
}

#[test]
fn reader_errors_are_typed_and_abort_provisional_hits() {
    let mut reader = FailingReader { sent_data: false };
    let mut sink = CollectSink::default();
    let error = scan(&mut reader, &options(2, &[Encoding::ASCII]), &mut sink).unwrap_err();

    assert!(matches!(error, ScanError::Reader(_)));
    assert_eq!(sink.aborted.len(), 1);
    assert!(sink.active.is_empty());
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum Method {
    Start,
    Chunk,
    Finish,
    Abort,
}

struct FailOnSink {
    fail_on: Method,
    calls: Vec<Method>,
}

impl StringSink for FailOnSink {
    type Error = TestError;

    fn start(&mut self, _id: HitId, _start: HitStart) -> Result<SinkControl, Self::Error> {
        self.call(Method::Start)?;
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, _id: HitId, _text: &str) -> Result<SinkControl, Self::Error> {
        self.call(Method::Chunk)?;
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, _id: HitId, _finish: HitFinish) -> Result<(), Self::Error> {
        self.call(Method::Finish)
    }

    fn abort(&mut self, _id: HitId) -> Result<(), Self::Error> {
        self.call(Method::Abort)
    }
}

impl FailOnSink {
    fn call(&mut self, method: Method) -> Result<(), TestError> {
        self.calls.push(method);
        if method == self.fail_on {
            Err(TestError("injected sink failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn no_sink_callback_follows_start_chunk_or_finish_error() {
    for (fail_on, expected) in [
        (Method::Start, vec![Method::Start]),
        (Method::Chunk, vec![Method::Start, Method::Chunk]),
        (
            Method::Finish,
            vec![Method::Start, Method::Chunk, Method::Chunk, Method::Finish],
        ),
    ] {
        let mut reader = &b"abc\0"[..];
        let mut sink = FailOnSink {
            fail_on,
            calls: Vec::new(),
        };
        let error = scan(&mut reader, &options(2, &[Encoding::ASCII]), &mut sink).unwrap_err();
        assert!(matches!(
            error,
            ScanError::Sink(TestError("injected sink failure"))
        ));
        assert_eq!(sink.calls, expected);
    }
}

#[test]
fn no_sink_callback_follows_abort_error() {
    let mut reader = FailingReader { sent_data: false };
    let mut sink = FailOnSink {
        fail_on: Method::Abort,
        calls: Vec::new(),
    };
    let error = scan(
        &mut reader,
        &options(2, &[Encoding::ASCII, Encoding::UTF16LE]),
        &mut sink,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ScanError::Sink(TestError("injected sink failure"))
    ));
    assert_eq!(sink.calls.last(), Some(&Method::Abort));
    assert_eq!(
        sink.calls
            .iter()
            .filter(|call| **call == Method::Abort)
            .count(),
        1
    );
}

#[derive(Default)]
struct CountingSink {
    chunks: u64,
    largest_chunk: usize,
    finish: Option<HitFinish>,
}

impl StringSink for CountingSink {
    type Error = TestError;

    fn start(&mut self, _id: HitId, _start: HitStart) -> Result<SinkControl, Self::Error> {
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, _id: HitId, text: &str) -> Result<SinkControl, Self::Error> {
        self.chunks += 1;
        self.largest_chunk = self.largest_chunk.max(text.len());
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, _id: HitId, finish: HitFinish) -> Result<(), Self::Error> {
        self.finish = Some(finish);
        Ok(())
    }

    fn abort(&mut self, _id: HitId) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn candidate_size_does_not_change_scanner_chunk_memory() {
    const LENGTH: u64 = 4 * 1024 * 1024;
    let mut reader = io::repeat(b'A').take(LENGTH);
    let mut sink = CountingSink::default();
    scan(&mut reader, &options(4, &[Encoding::ASCII]), &mut sink).unwrap();

    assert_eq!(sink.finish.unwrap().character_count, LENGTH);
    assert_eq!(sink.largest_chunk, 4);
    assert_eq!(sink.chunks, LENGTH - 3);
}

/// Asserts the complete set of shadowed readings as `(text, offset, encoding)`.
#[track_caller]
fn assert_shadowed_readings(sink: &CollectSink, expected: &[(String, u64, Encoding)]) {
    let key = |reading: &(String, u64, Encoding)| (reading.1, reading.2.to_string());
    let mut actual = sink
        .records
        .iter()
        .filter(|record| record.finish.shadowed)
        .map(|record| {
            (
                record.text.clone(),
                record.start.offset.get(),
                record.start.encoding,
            )
        })
        .collect::<Vec<_>>();
    actual.sort_by_key(key);
    let mut expected = expected.to_vec();
    expected.sort_by_key(key);
    assert_eq!(actual, expected);
}

#[test]
fn odd_aligned_utf16_shadows_shifted_artifacts() {
    let bytes = utf16("test", false, 1);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(3, &[Encoding::UTF16LE, Encoding::UTF16BE]),
        &mut sink,
    )
    .unwrap();

    assert_unique_winner(&sink, "test", 1, Encoding::UTF16LE);
    // Bytes ff 74 00 65 00 73 00 74: little-endian pairs from offset 0 read U+74FF and the
    // swapped letters; big-endian pairs read U+FF74 and "est".
    assert_shadowed_readings(
        &sink,
        &[
            (format!("\u{74ff}{}", swapped("est")), 0, Encoding::UTF16LE),
            ("\u{ff74}est".into(), 0, Encoding::UTF16BE),
        ],
    );
}

/// Asserts that exactly one unshadowed record holds `text`, at `offset` from `encoding`.
#[track_caller]
fn assert_unique_winner(sink: &CollectSink, text: &str, offset: u64, encoding: Encoding) {
    let matches = sink
        .records
        .iter()
        .filter(|record| record.text == text && !record.finish.shadowed)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "{text:?}: {matches:?}");
    assert_eq!(matches[0].start.offset.get(), offset);
    assert_eq!(matches[0].start.encoding, encoding);
}

const LARGE_LENGTH: usize = 64 * 1024 + 1;

fn oversized_neighbor_fixture(big_endian: bool) -> (Vec<u8>, u64) {
    let mut bytes = Vec::new();
    append_utf16(&mut bytes, "MATCH_A", big_endian, true);
    for _ in 0..LARGE_LENGTH {
        bytes.extend(if big_endian {
            u16::from(b'X').to_be_bytes()
        } else {
            u16::from(b'X').to_le_bytes()
        });
    }
    bytes.extend(if big_endian {
        0_u16.to_be_bytes()
    } else {
        0_u16.to_le_bytes()
    });
    let match_b_offset = bytes.len() as u64;
    append_utf16(&mut bytes, "MATCH_B", big_endian, true);
    (bytes, match_b_offset)
}

#[test]
fn utf16le_large_neighbor_prefers_exact_candidate_over_shifted_be_copy() {
    let (bytes, match_b_offset) = oversized_neighbor_fixture(false);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(4, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE]),
        &mut sink,
    )
    .unwrap();

    assert_unique_winner(&sink, "MATCH_A", 0, Encoding::UTF16LE);
    assert_unique_winner(&sink, "MATCH_B", match_b_offset, Encoding::UTF16LE);
    // "MATCH_A" and its NUL fill bytes 0..16, so the X run starts at 16.
    let x_run = "X".repeat(LARGE_LENGTH);
    assert_shadowed_readings(
        &sink,
        &[
            (swapped("ATCH_A"), 1, Encoding::UTF16LE),
            ("ATCH_A".into(), 1, Encoding::UTF16BE),
            (swapped(&x_run), 15, Encoding::UTF16LE),
            (x_run, 15, Encoding::UTF16BE),
            (swapped("MATCH_B"), match_b_offset - 1, Encoding::UTF16LE),
            ("MATCH_B".into(), match_b_offset - 1, Encoding::UTF16BE),
        ],
    );
}

#[test]
fn utf16be_large_neighbor_prefers_exact_candidate_over_shifted_le_copy() {
    let (bytes, match_b_offset) = oversized_neighbor_fixture(true);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(4, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE]),
        &mut sink,
    )
    .unwrap();

    // Each big-endian hit is evidence for the next one, two bytes after it. The shadowed
    // little-endian X run must not become evidence for its lane, or the little-endian
    // "MATCH_B" copy would tie and win.
    assert_unique_winner(&sink, "MATCH_A", 0, Encoding::UTF16BE);
    assert_unique_winner(&sink, "MATCH_B", match_b_offset, Encoding::UTF16BE);
    let x_run = "X".repeat(LARGE_LENGTH);
    assert_shadowed_readings(
        &sink,
        &[
            (swapped("MATCH_A"), 0, Encoding::UTF16LE),
            ("MATCH_A".into(), 1, Encoding::UTF16LE),
            (swapped("MATCH_A"), 1, Encoding::UTF16BE),
            (swapped(&x_run), 16, Encoding::UTF16LE),
            (x_run.clone(), 17, Encoding::UTF16LE),
            (swapped(&x_run), 17, Encoding::UTF16BE),
            (swapped("MATCH_B"), match_b_offset, Encoding::UTF16LE),
            ("MATCH_B".into(), match_b_offset + 1, Encoding::UTF16LE),
            (swapped("MATCH_B"), match_b_offset + 1, Encoding::UTF16BE),
        ],
    );
}

#[test]
fn genuine_odd_aligned_utf16_wins_exact_copy_shadowing() {
    let le_bytes = utf16("ODD_REAL", false, 1);
    let be_bytes = utf16("ODD_REAL", true, 1);
    for (bytes, encoding, shadowed) in [
        // ff 4f 00 44 00 ...: pairs from offset 0 read U+4FFF or U+FF4F first.
        (
            le_bytes,
            Encoding::UTF16LE,
            vec![
                (
                    format!("\u{4fff}{}", swapped("DD_REAL")),
                    0,
                    Encoding::UTF16LE,
                ),
                ("\u{ff4f}DD_REAL".to_owned(), 0, Encoding::UTF16BE),
            ],
        ),
        // ff 00 4f 00 44 ... 4c 00 00: pairs from offset 0 read U+00FF or U+FF00 first, and
        // the little-endian view reaches the terminator's first byte. Little-endian pairs from
        // offset 1 (00 4f, 00 44, ...) read the same U+4F00, U+4400, ... as big-endian pairs
        // from offset 0 (4f 00, 44 00, ...), and the open view from offset 0 shadows them.
        (
            be_bytes,
            Encoding::UTF16BE,
            vec![
                ("\u{ff}ODD_REAL".to_owned(), 0, Encoding::UTF16LE),
                (
                    format!("\u{ff00}{}", swapped("ODD_REAL")),
                    0,
                    Encoding::UTF16BE,
                ),
                (swapped("ODD_REAL"), 1, Encoding::UTF16LE),
            ],
        ),
    ] {
        let mut reader = bytes.as_slice();
        let mut sink = CollectSink::default();
        scan(
            &mut reader,
            &options(4, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE]),
            &mut sink,
        )
        .unwrap();

        assert_unique_winner(&sink, "ODD_REAL", 1, encoding);
        assert_shadowed_readings(&sink, &shadowed);
    }
}

#[test]
fn eof_candidate_prefers_exact_encoding_without_hiding_its_neighbor() {
    // "LEFT" and its NUL fill bytes 0..10, so "TARGET" starts at 10 and runs to EOF at 22.
    let le_shadowed = [
        (swapped("TARGET"), 9, Encoding::UTF16LE),
        ("TARGET".to_owned(), 9, Encoding::UTF16BE),
    ];
    let be_shadowed = [
        (swapped("LEFT"), 0, Encoding::UTF16LE),
        ("LEFT".to_owned(), 1, Encoding::UTF16LE),
        (swapped("LEFT"), 1, Encoding::UTF16BE),
        (swapped("TARGET"), 10, Encoding::UTF16LE),
        // The odd lane reaches EOF one byte short of the final "T".
        ("TARGE".to_owned(), 11, Encoding::UTF16LE),
        (swapped("TARGE"), 11, Encoding::UTF16BE),
    ];
    for (encoding, big_endian, shadowed) in [
        (Encoding::UTF16LE, false, &le_shadowed[..]),
        (Encoding::UTF16BE, true, &be_shadowed[..]),
    ] {
        let mut bytes = Vec::new();
        append_utf16(&mut bytes, "LEFT", big_endian, true);
        let target_offset = bytes.len() as u64;
        append_utf16(&mut bytes, "TARGET", big_endian, false);
        let mut reader = bytes.as_slice();
        let mut sink = CollectSink::default();
        scan(
            &mut reader,
            &options(4, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE]),
            &mut sink,
        )
        .unwrap();

        assert_unique_winner(&sink, "LEFT", 0, encoding);
        assert_unique_winner(&sink, "TARGET", target_offset, encoding);
        assert_shadowed_readings(&sink, shadowed);
    }
}

#[test]
fn exact_opposite_endian_copy_without_evidence_prefers_little_endian() {
    // 00 00 | 00 52 00 69 00 63 00 68 | 00 00 00: big-endian "Rich" at 2 and little-endian
    // "Rich" at 3. Neither starts the input or follows a hit of its lane.
    let bytes = b"\0\0\0R\0i\0c\0h\0\0\0";
    for encodings in [
        [Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE],
        [Encoding::ASCII, Encoding::UTF16BE, Encoding::UTF16LE],
    ] {
        let mut reader = &bytes[..];
        let mut sink = CollectSink::default();
        scan(&mut reader, &options(4, &encodings), &mut sink).unwrap();

        assert_unique_winner(&sink, "Rich", 3, Encoding::UTF16LE);
        assert_shadowed_readings(
            &sink,
            &[
                (swapped("Rich"), 2, Encoding::UTF16LE),
                ("Rich".into(), 2, Encoding::UTF16BE),
            ],
        );
    }
}

fn utf16_options() -> ScanOptions {
    options(4, &[Encoding::ASCII, Encoding::UTF16LE, Encoding::UTF16BE])
}

#[test]
fn big_endian_copy_of_little_endian_cyrillic_is_shadowed() {
    // "Пароль: %s" in UTF-16LE at 0x28: 1f 04 30 04 40 04 3e 04 3b 04 4c 04 3a 00 20 00 25 00
    // 73 00 00 00. Big-endian pairs from 0x29 are 04 30, 04 40, 04 3e, 04 3b, 04 4c, 04 3a,
    // 00 20, 00 25, 00 73: "арольк %s", where 04 3a is U+043A.
    let mut bytes = vec![0; 0x28];
    bytes.extend([
        0x1f, 0x04, 0x30, 0x04, 0x40, 0x04, 0x3e, 0x04, 0x3b, 0x04, 0x4c, 0x04, 0x3a, 0x00, 0x20,
        0x00, 0x25, 0x00, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(&mut reader, &utf16_options(), &mut sink).unwrap();

    assert_unique_winner(&sink, "Пароль: %s", 0x28, Encoding::UTF16LE);
    assert_shadowed(&sink, "арольк %s", 0x29, Encoding::UTF16BE);
}

#[test]
fn big_endian_cyrillic_with_evidence_shadows_its_little_endian_copy() {
    // UTF-16BE "Файл" at 0, one NUL code unit, then UTF-16BE "Пароль" at 10:
    // 04 24 04 30 04 39 04 3b 00 00 04 1f 04 30 04 40 04 3e 04 3b 04 4c 00 00.
    let bytes = [
        0x04, 0x24, 0x04, 0x30, 0x04, 0x39, 0x04, 0x3b, 0x00, 0x00, 0x04, 0x1f, 0x04, 0x30, 0x04,
        0x40, 0x04, 0x3e, 0x04, 0x3b, 0x04, 0x4c, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut reader = &bytes[..];
    let mut sink = CollectSink::default();
    scan(&mut reader, &utf16_options(), &mut sink).unwrap();

    // "Файл" starts the input and "Пароль" follows it after one NUL code unit, so the
    // big-endian lane has evidence both times.
    assert_unique_winner(&sink, "Файл", 0, Encoding::UTF16BE);
    assert_unique_winner(&sink, "Пароль", 10, Encoding::UTF16BE);
    // Little-endian pairs from 1 are 24 04, 30 04, 39 04, 3b 00: "Фай;", ended when "Файл"
    // wins. The lane continues with 00 04 (U+0400), 1f 04, ..., 4c 00 (U+004C).
    assert_shadowed(&sink, "Фай;", 1, Encoding::UTF16LE);
    assert_shadowed(&sink, "\u{400}ПаролL", 9, Encoding::UTF16LE);
}

#[test]
fn split_big_endian_copy_of_a_mixed_script_path_is_shadowed() {
    // "C:\Users\Пользователь" in UTF-16LE at 16, 42 bytes.
    let mut bytes = vec![0; 16];
    bytes.extend(b"C\0:\0\\\0U\0s\0e\0r\0s\0\\\0");
    bytes.extend([
        0x1f, 0x04, 0x3e, 0x04, 0x3b, 0x04, 0x4c, 0x04, 0x37, 0x04, 0x3e, 0x04, 0x32, 0x04, 0x30,
        0x04, 0x42, 0x04, 0x35, 0x04, 0x3b, 0x04, 0x4c, 0x04, 0x00, 0x00, 0x00, 0x00,
    ]);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(&mut reader, &utf16_options(), &mut sink).unwrap();

    assert_unique_winner(&sink, "C:\\Users\\Пользователь", 16, Encoding::UTF16LE);
    // Big-endian pairs from 15 start with the zero byte before the path: 00 43, 00 3a, ...,
    // 00 5c read "C:\Users\" until 00 1f, the control U+001F at 33. The lane restarts at 35
    // with 04 3e, ..., 04 4c, then 04 00 (U+0400) from the last high byte and the terminator.
    assert_shadowed(&sink, "C:\\Users\\", 15, Encoding::UTF16BE);
    assert_shadowed(&sink, "ользователь\u{400}", 35, Encoding::UTF16BE);
}

#[test]
fn cjk_reading_that_ascii_wins_is_reported_shadowed() {
    let scan_options = options(
        4,
        &[
            Encoding::ASCII,
            Encoding::UTF8,
            Encoding::UTF16LE,
            Encoding::UTF16BE,
        ],
    );

    // Every UTF-16LE byte of U+4E2D U+6C49 U+5B57 U+5927 U+5728 U+4E2D U+6C49 U+5B57 is
    // printable ASCII: 2d 4e 49 6c 57 5b 27 59 28 57 2d 4e 49 6c 57 5b.
    let mut bytes = vec![0; 16];
    bytes.extend([
        0x2d, 0x4e, 0x49, 0x6c, 0x57, 0x5b, 0x27, 0x59, 0x28, 0x57, 0x2d, 0x4e, 0x49, 0x6c, 0x57,
        0x5b,
    ]);
    bytes.extend([0; 18]);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    scan(&mut reader, &scan_options, &mut sink).unwrap();

    assert_unique_winner(&sink, "-NIlW['Y(W-NIlW[", 16, Encoding::ASCII);
    assert_shadowed(&sink, "中汉字大在中汉字", 16, Encoding::UTF16LE);
    assert_eq!(
        matching_record(&sink, "中汉字大在中汉字", 16)
            .finish
            .source_length
            .get(),
        16
    );

    // "ください" in UTF-16LE, U+304F U+3060 U+3055 U+3044: every byte is printable ASCII.
    let bytes = [0, 0, 0x4f, 0x30, 0x60, 0x30, 0x55, 0x30, 0x44, 0x30, 0, 0];
    let mut reader = &bytes[..];
    let mut sink = CollectSink::default();
    scan(&mut reader, &scan_options, &mut sink).unwrap();

    assert_unique_winner(&sink, "O0`0U0D0", 2, Encoding::ASCII);
    assert_shadowed(&sink, "ください", 2, Encoding::UTF16LE);
}

#[test]
fn utf8_reports_non_ascii_text_at_byte_offsets() {
    let bytes = [
        0x01, // "Привет": U+041F U+0440 U+0438 U+0432 U+0435 U+0442.
        0xd0, 0x9f, 0xd1, 0x80, 0xd0, 0xb8, 0xd0, 0xb2, 0xd0, 0xb5, 0xd1, 0x82, 0x00,
        // "中文字": U+4E2D U+6587 U+5B57.
        0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87, 0xe5, 0xad, 0x97, 0x0a,
    ];
    let mut reader = &bytes[..];
    let mut sink = CollectSink::default();
    let summary = scan(
        &mut reader,
        &options(3, &[Encoding::ASCII, Encoding::UTF8]),
        &mut sink,
    )
    .unwrap();

    let russian = matching_record(&sink, "Привет", 1);
    assert_eq!(russian.start.encoding, Encoding::UTF8);
    assert_eq!(russian.finish.source_length.get(), 12);
    assert_eq!(russian.finish.character_count, 6);
    assert_eq!(russian.finish.decoded_utf8_length, 12);
    assert!(!russian.finish.shadowed);
    let chinese = matching_record(&sink, "中文字", 14);
    assert_eq!(chinese.start.encoding, Encoding::UTF8);
    assert_eq!(chinese.finish.source_length.get(), 9);
    assert_eq!(chinese.finish.character_count, 3);
    assert_eq!(sink.records.len(), 2, "{:?}", sink.records);
    assert_eq!(summary.candidate_count, 2);
}

#[test]
fn utf8_reads_a_non_ascii_path_whole_beside_the_ascii_pieces() {
    // "ö" is c3 b6, so the ASCII decoder stops at byte 10 and restarts at byte 12.
    let bytes = b"C:\\Users\\J\xc3\xb6rg\\Desktop\\a.txt\0";
    let mut reader = &bytes[..];
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(3, &[Encoding::ASCII, Encoding::UTF8]),
        &mut sink,
    )
    .unwrap();

    let path = matching_record(&sink, "C:\\Users\\Jörg\\Desktop\\a.txt", 0);
    assert_eq!(path.start.encoding, Encoding::UTF8);
    assert_eq!(path.finish.source_length.get(), 28);
    assert_eq!(path.finish.character_count, 27);
    assert_eq!(
        matching_record(&sink, "C:\\Users\\J", 0).start.encoding,
        Encoding::ASCII
    );
    assert_eq!(
        matching_record(&sink, "rg\\Desktop\\a.txt", 12)
            .start
            .encoding,
        Encoding::ASCII
    );
    assert_eq!(sink.records.len(), 3, "{:?}", sink.records);
    assert!(sink.records.iter().all(|record| !record.finish.shadowed));
}

/// Scans with ASCII and UTF-8 and returns the records as `(text, offset, encoding)`.
fn utf8_readings(bytes: &[u8], min_length: usize) -> Vec<(String, u64, Encoding)> {
    let mut reader = bytes;
    let mut sink = CollectSink::default();
    scan(
        &mut reader,
        &options(min_length, &[Encoding::ASCII, Encoding::UTF8]),
        &mut sink,
    )
    .unwrap();
    let mut readings = sink
        .records
        .into_iter()
        .map(|record| {
            (
                record.text,
                record.start.offset.get(),
                record.start.encoding,
            )
        })
        .collect::<Vec<_>>();
    readings.sort_by_key(|reading| reading.1);
    readings
}

#[test]
fn ill_formed_utf8_ends_a_candidate_and_is_never_decoded() {
    for ill_formed in [
        &[0xc0, 0x80][..],                     // overlong U+0000
        &[0xc1, 0x81],                         // overlong "A"
        &[0xe0, 0x81, 0x81],                   // overlong "A"
        &[0xf0, 0x80, 0x81, 0x81],             // overlong "A"
        &[0xed, 0xa0, 0x80],                   // surrogate U+D800
        &[0xed, 0xbf, 0xbf],                   // surrogate U+DFFF
        &[0xf4, 0x90, 0x80, 0x80],             // U+110000
        &[0xf5, 0x80, 0x80, 0x80],             // lead above F4
        &[0xf8, 0x88, 0x80, 0x80, 0x80],       // five-byte lead
        &[0xfc, 0x84, 0x80, 0x80, 0x80, 0x80], // six-byte lead
        &[0x80],                               // stray continuation
        &[0xbf, 0xbf],                         // stray continuations
    ] {
        // "Jö" is 4a c3 b6 and "ün" is c3 bc 6e.
        let mut bytes = b"J\xc3\xb6".to_vec();
        bytes.extend(ill_formed);
        bytes.extend(b"\xc3\xbcn");

        assert_eq!(
            utf8_readings(&bytes, 2),
            vec![
                ("Jö".to_owned(), 0, Encoding::UTF8),
                ("ün".to_owned(), 3 + ill_formed.len() as u64, Encoding::UTF8),
            ],
            "{ill_formed:02x?}"
        );
    }
}

#[test]
fn utf8_resynchronizes_inside_a_truncated_sequence() {
    // "中文字" is e4 b8 ad e6 96 87 e5 ad 97.
    let text = [0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87, 0xe5, 0xad, 0x97];
    for truncated in [
        &[0xe4, 0xb8][..],   // U+4E2D without its last byte
        &[0xf0, 0x9f, 0x98], // U+1F600 without its last byte
        &[0xe0],             // a lead whose next byte must be a0..bf
        &[0xc3],             // a two-byte lead
    ] {
        let mut bytes = b"J\xc3\xb6".to_vec();
        bytes.extend(truncated);
        bytes.extend(text);

        assert_eq!(
            utf8_readings(&bytes, 2),
            vec![
                ("Jö".to_owned(), 0, Encoding::UTF8),
                (
                    "中文字".to_owned(),
                    3 + truncated.len() as u64,
                    Encoding::UTF8
                ),
            ],
            "{truncated:02x?}"
        );
    }
}

#[test]
fn all_ascii_utf8_candidate_is_reported_once_by_the_ascii_decoder() {
    let mut reader = &b"plain ascii\0"[..];
    let mut sink = CollectSink::default();
    let summary = scan(
        &mut reader,
        &options(3, &[Encoding::UTF8, Encoding::ASCII]),
        &mut sink,
    )
    .unwrap();

    assert_eq!(sink.records.len(), 1, "{:?}", sink.records);
    assert_eq!(
        matching_record(&sink, "plain ascii", 0).start.encoding,
        Encoding::ASCII
    );
    assert_eq!(sink.aborted.len(), 1);
    assert_eq!(summary.candidate_count, 1);

    // Without the ASCII decoder the UTF-8 reading is not a duplicate, so it is reported.
    let mut reader = &b"plain ascii\0"[..];
    let mut sink = CollectSink::default();
    scan(&mut reader, &options(3, &[Encoding::UTF8]), &mut sink).unwrap();

    assert_eq!(sink.records.len(), 1, "{:?}", sink.records);
    assert_eq!(
        matching_record(&sink, "plain ascii", 0).start.encoding,
        Encoding::UTF8
    );
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum Event {
    Start(u64, Encoding, u64),
    Chunk(u64, String),
    /// Id, source length, character count, decoded UTF-8 length, and shadowed.
    Finish(u64, u64, u64, u64, bool),
    Abort(u64),
}

#[derive(Default)]
struct EventSink(Vec<Event>);

impl StringSink for EventSink {
    type Error = TestError;

    fn start(&mut self, id: HitId, start: HitStart) -> Result<SinkControl, Self::Error> {
        self.0
            .push(Event::Start(id.get(), start.encoding, start.offset.get()));
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, id: HitId, text: &str) -> Result<SinkControl, Self::Error> {
        self.0.push(Event::Chunk(id.get(), text.to_owned()));
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, id: HitId, finish: HitFinish) -> Result<(), Self::Error> {
        self.0.push(Event::Finish(
            id.get(),
            finish.source_length.get(),
            finish.character_count,
            finish.decoded_utf8_length,
            finish.shadowed,
        ));
        Ok(())
    }

    fn abort(&mut self, id: HitId) -> Result<(), Self::Error> {
        self.0.push(Event::Abort(id.get()));
        Ok(())
    }
}

#[test]
fn overlap_events_follow_decoder_and_terminal_precedence_order() {
    let mut reader = &b"ABCD\0"[..];
    let mut sink = EventSink::default();
    scan(
        &mut reader,
        &options(2, &[Encoding::ASCII, Encoding::UTF16LE]),
        &mut sink,
    )
    .unwrap();

    assert_eq!(
        sink.0,
        vec![
            Event::Start(0, Encoding::ASCII, 0),
            Event::Chunk(0, "AB".into()),
            Event::Chunk(0, "C".into()),
            Event::Chunk(0, "D".into()),
            Event::Start(1, Encoding::UTF16LE, 0),
            Event::Chunk(1, "䉁䑃".into()),
            // The ASCII hit shadows the UTF-16 view of bytes 0..4 before it finishes itself.
            Event::Finish(1, 4, 2, 6, true),
            Event::Finish(0, 4, 4, 4, false),
            Event::Start(2, Encoding::UTF16LE, 1),
            Event::Chunk(2, "䍂D".into()),
            Event::Finish(2, 4, 2, 4, true),
        ]
    );
}

fn event_stream<R: Read>(mut reader: R, scan_options: &ScanOptions) -> (Vec<Event>, ScanSummary) {
    let mut sink = EventSink::default();
    let summary = scan(&mut reader, scan_options, &mut sink).unwrap();
    (sink.0, summary)
}

#[test]
fn ambiguous_non_ascii_overlaps_are_retained_with_ground_truth_order() {
    let bytes = [0x01, 0x4e, 0x02, 0x4e, 0x03, 0x4e, 0x00, 0x00];
    let scan_options = options(3, &[Encoding::UTF16LE]);
    let (events, summary) = event_stream(bytes.as_slice(), &scan_options);
    let repeated = event_stream(bytes.as_slice(), &scan_options);

    assert_eq!(repeated, (events.clone(), summary));

    assert_eq!(
        events,
        vec![
            Event::Start(0, Encoding::UTF16LE, 0),
            Event::Chunk(0, "\u{4e01}\u{4e02}\u{4e03}".into()),
            Event::Start(1, Encoding::UTF16LE, 1),
            Event::Chunk(1, "\u{024e}\u{034e}N".into()),
            Event::Finish(0, 6, 3, 9, false),
            Event::Finish(1, 6, 3, 5, false),
        ]
    );
    assert_eq!(summary.candidate_count, 2);
}

#[test]
fn short_reads_match_the_complete_one_shot_event_stream() {
    let mut bytes = b"ASCII evidence\0".to_vec();
    bytes.extend(utf16("odd é😀", false, 1));
    bytes.extend([0xd8, 0x00, b'O', 0, b'K', 0, 0, 0]);
    let utf8_offset = bytes.len() as u64;
    bytes.extend("Jörg 中文 😀\0".as_bytes());
    bytes.extend([0xe4, 0xb8, b'O', b'K', 0]);
    let scan_options = options(
        2,
        &[
            Encoding::ASCII,
            Encoding::UTF8,
            Encoding::UTF16LE,
            Encoding::UTF16BE,
        ],
    );
    let expected = event_stream(bytes.as_slice(), &scan_options);
    assert!(expected.0.iter().any(|event| matches!(
        event,
        Event::Start(_, Encoding::UTF8, offset) if *offset == utf8_offset
    )));
    assert!(expected
        .0
        .iter()
        .any(|event| matches!(event, Event::Abort(_))));

    for chunk_size in [1, 2, 3] {
        let actual = event_stream(
            ShortReader {
                bytes: &bytes,
                chunk_size,
            },
            &scan_options,
        );
        assert_eq!(actual, expected, "chunk size {chunk_size}");
    }
}

struct PatternReader<'a> {
    bytes: &'a [u8],
    pattern: &'a [usize],
    next: usize,
}

impl Read for PatternReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.bytes.is_empty() {
            return Ok(0);
        }
        let requested = self.pattern[self.next % self.pattern.len()];
        self.next += 1;
        let length = requested.min(self.bytes.len()).min(buffer.len());
        buffer[..length].copy_from_slice(&self.bytes[..length]);
        self.bytes = &self.bytes[length..];
        Ok(length)
    }
}

#[test]
fn deterministic_property_chunking_preserves_all_events() {
    let scan_options = options(
        3,
        &[
            Encoding::ASCII,
            Encoding::UTF8,
            Encoding::UTF16LE,
            Encoding::UTF16BE,
        ],
    );
    for seed in 0_u64..32 {
        let mut state = seed + 1;
        let bytes = (0..257)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 32) as u8
            })
            .collect::<Vec<_>>();
        let pattern = [1, (seed as usize % 7) + 1, (seed as usize % 17) + 1, 3, 29];
        let expected = event_stream(bytes.as_slice(), &scan_options);
        let actual = event_stream(
            PatternReader {
                bytes: &bytes,
                pattern: &pattern,
                next: 0,
            },
            &scan_options,
        );
        assert_eq!(actual, expected, "seed {seed}");
    }
}
