use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::io::{self, Read};

use rust_strings::{
    scan, Encoding, HitFinish, HitId, HitStart, ScanError, ScanOptions, ScanOptionsError,
    SinkControl, StringSink,
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

fn matching_record<'a>(sink: &'a CollectSink, text: &str, offset: u64) -> &'a Record {
    sink.records
        .iter()
        .find(|record| record.text == text && record.start.offset.get() == offset)
        .unwrap_or_else(|| panic!("missing record {text:?} at {offset}: {:?}", sink.records))
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

#[test]
fn odd_aligned_utf16_wins_over_shifted_artifacts() {
    let bytes = utf16("test", false, 1);
    let mut reader = bytes.as_slice();
    let mut sink = CollectSink::default();
    let summary = scan(
        &mut reader,
        &options(3, &[Encoding::UTF16LE, Encoding::UTF16BE]),
        &mut sink,
    )
    .unwrap();

    let matches = sink
        .records
        .iter()
        .filter(|record| record.text == "test" && record.start.offset.get() == 1)
        .count();
    assert_eq!(matches, 1);
    assert!(summary.suppressed_candidate_count > 0);
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum Event {
    Start(u64, Encoding, u64),
    Chunk(u64, String),
    Finish(u64),
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

    fn finish(&mut self, id: HitId, _finish: HitFinish) -> Result<(), Self::Error> {
        self.0.push(Event::Finish(id.get()));
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
            Event::Abort(1),
            Event::Finish(0),
            Event::Start(2, Encoding::UTF16LE, 1),
            Event::Chunk(2, "䍂D".into()),
            Event::Abort(2),
        ]
    );
}
