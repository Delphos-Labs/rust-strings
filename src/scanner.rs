use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::num::NonZeroUsize;

use crate::Encoding;

const READ_BUFFER_SIZE: usize = 8 * 1024;

/// Identifies one provisional hit for its complete sink lifetime.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct HitId(u64);

impl HitId {
    /// Returns the numeric identifier.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A byte offset in the source input.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SourceOffset(u64);

impl SourceOffset {
    /// Returns the numeric byte offset.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A byte length in the source input.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SourceLength(u64);

impl SourceLength {
    /// Returns the numeric byte length.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Metadata available when a provisional hit starts.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct HitStart {
    /// The first source byte in the candidate.
    pub offset: SourceOffset,
    /// The decoder that produced the candidate.
    pub encoding: Encoding,
}

/// Metadata available when a hit becomes valid.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct HitFinish {
    /// The source bytes that contain valid decoded characters.
    pub source_length: SourceLength,
    /// The number of decoded Unicode scalar values.
    pub character_count: u64,
}

/// Controls delivery for the hit associated with the current callback.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SinkControl {
    /// Continue to deliver decoded text chunks.
    Continue,
    /// Stop text delivery, but continue scanning and send finish or abort.
    SkipCurrent,
}

/// Receives interleaved events from all active decoder candidates.
///
/// A successful `start` is followed by exactly one `finish` or `abort` with
/// the same [`HitId`]. A hit is provisional until `finish` succeeds.
pub trait StringSink {
    /// An error returned by this sink.
    type Error;

    /// Starts a provisional hit.
    fn start(&mut self, id: HitId, hit: HitStart) -> Result<SinkControl, Self::Error>;
    /// Receives a valid UTF-8 part of a provisional hit.
    fn chunk(&mut self, id: HitId, text: &str) -> Result<SinkControl, Self::Error>;
    /// Marks a provisional hit as valid and complete.
    fn finish(&mut self, id: HitId, hit: HitFinish) -> Result<(), Self::Error>;
    /// Discards a provisional hit.
    fn abort(&mut self, id: HitId) -> Result<(), Self::Error>;
}

/// Validated scanner settings.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ScanOptions {
    min_length: NonZeroUsize,
    encodings: Vec<Encoding>,
}

impl ScanOptions {
    /// Validates a nonzero minimum and at least one selected encoding.
    pub fn new(
        min_length: usize,
        encodings: impl IntoIterator<Item = Encoding>,
    ) -> Result<Self, ScanOptionsError> {
        let min_length =
            NonZeroUsize::new(min_length).ok_or(ScanOptionsError::ZeroMinimumLength)?;
        let mut selected = Vec::new();
        for encoding in encodings {
            if !selected.contains(&encoding) {
                selected.push(encoding);
            }
        }
        if selected.is_empty() {
            return Err(ScanOptionsError::NoEncodings);
        }
        Ok(Self {
            min_length,
            encodings: selected,
        })
    }

    /// Returns the minimum number of decoded scalar values in a hit.
    pub fn min_length(&self) -> usize {
        self.min_length.get()
    }

    /// Returns the selected encodings in scan order.
    pub fn encodings(&self) -> &[Encoding] {
        &self.encodings
    }
}

/// An invalid scanner setting.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ScanOptionsError {
    /// The minimum candidate length was zero.
    ZeroMinimumLength,
    /// The encoding list was empty.
    NoEncodings,
}

impl fmt::Display for ScanOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMinimumLength => f.write_str("minimum length must be greater than zero"),
            Self::NoEncodings => f.write_str("at least one encoding must be selected"),
        }
    }
}

impl Error for ScanOptionsError {}

/// A failure from the caller-owned reader or sink.
#[derive(Debug)]
pub enum ScanError<E> {
    /// The reader failed.
    Reader(io::Error),
    /// The sink failed.
    Sink(E),
}

impl<E: fmt::Display> fmt::Display for ScanError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(error) => write!(f, "reader error: {error}"),
            Self::Sink(error) => write!(f, "sink error: {error}"),
        }
    }
}

impl<E> Error for ScanError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::Sink(error) => Some(error),
        }
    }
}

/// Counts produced by one complete scan.
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct ScanSummary {
    /// The number of source bytes consumed.
    pub bytes_read: u64,
    /// The number of candidates completed through `StringSink::finish`.
    pub candidate_count: u64,
    /// The completed candidates for which the sink skipped text.
    pub skipped_candidate_count: u64,
    /// The provisional shifted UTF-16 artifacts discarded through `abort`.
    pub suppressed_candidate_count: u64,
}

#[derive(Debug)]
struct Candidate {
    start: u64,
    end: u64,
    character_count: u64,
    ascii_character_count: u64,
    prefix: String,
    open: Option<OpenHit>,
}

impl Candidate {
    fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            character_count: 0,
            ascii_character_count: 0,
            prefix: String::new(),
            open: None,
        }
    }

    fn reset(&mut self) {
        self.start = 0;
        self.end = 0;
        self.character_count = 0;
        self.ascii_character_count = 0;
        self.prefix.clear();
        self.open = None;
    }
}

#[derive(Debug, Copy, Clone)]
struct OpenHit {
    id: HitId,
    skipped: bool,
}

#[derive(Debug)]
enum DecoderKind {
    Ascii,
    Utf16 {
        big_endian: bool,
        alignment: u8,
        first_byte: Option<(u64, u8)>,
        high_surrogate: Option<(u64, u16)>,
    },
}

#[derive(Debug)]
struct Decoder {
    encoding: Encoding,
    kind: DecoderKind,
    candidate: Candidate,
}

struct Scanner<'a, S> {
    sink: &'a mut S,
    min_length: usize,
    decoders: Vec<Decoder>,
    next_hit_id: u64,
    summary: ScanSummary,
}

/// Streams decoded string candidates from a caller-owned reader to a sink.
///
/// The scanner keeps bounded read and decoder state. It buffers only the
/// configured minimum before it starts a hit.
pub fn scan<R, S>(
    reader: &mut R,
    options: &ScanOptions,
    sink: &mut S,
) -> Result<ScanSummary, ScanError<S::Error>>
where
    R: Read,
    S: StringSink,
{
    let mut scanner = Scanner::new(options, sink);
    let mut buffer = [0_u8; READ_BUFFER_SIZE];

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if let Err(sink_error) = scanner.abort_all() {
                    return Err(ScanError::Sink(sink_error));
                }
                return Err(ScanError::Reader(error));
            }
        };

        for byte in &buffer[..read] {
            let offset = scanner.summary.bytes_read;
            if let Err(error) = scanner.consume(offset, *byte) {
                scanner.abort_all_ignoring_errors();
                return Err(ScanError::Sink(error));
            }
            scanner.summary.bytes_read =
                scanner.summary.bytes_read.checked_add(1).ok_or_else(|| {
                    ScanError::Reader(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "input length exceeds u64",
                    ))
                })?;
        }
    }

    if let Err(error) = scanner.finish_eof() {
        scanner.abort_all_ignoring_errors();
        return Err(ScanError::Sink(error));
    }
    Ok(scanner.summary)
}

impl<'a, S: StringSink> Scanner<'a, S> {
    fn new(options: &ScanOptions, sink: &'a mut S) -> Self {
        let mut decoders = Vec::new();
        for encoding in &options.encodings {
            match encoding {
                Encoding::ASCII => decoders.push(Decoder {
                    encoding: *encoding,
                    kind: DecoderKind::Ascii,
                    candidate: Candidate::new(),
                }),
                Encoding::UTF16LE | Encoding::UTF16BE => {
                    for alignment in 0..=1 {
                        decoders.push(Decoder {
                            encoding: *encoding,
                            kind: DecoderKind::Utf16 {
                                big_endian: *encoding == Encoding::UTF16BE,
                                alignment,
                                first_byte: None,
                                high_surrogate: None,
                            },
                            candidate: Candidate::new(),
                        });
                    }
                }
            }
        }
        Self {
            sink,
            min_length: options.min_length(),
            decoders,
            next_hit_id: 0,
            summary: ScanSummary::default(),
        }
    }

    fn consume(&mut self, offset: u64, byte: u8) -> Result<(), S::Error> {
        for index in 0..self.decoders.len() {
            match self.decoders[index].kind {
                DecoderKind::Ascii => {
                    if is_ascii_string_byte(byte) {
                        self.add_character(index, offset, 1, char::from(byte))?;
                    } else {
                        self.finish_detected_candidate(index)?;
                    }
                }
                DecoderKind::Utf16 { .. } => {
                    if let Some((unit_offset, unit)) =
                        self.decoders[index].consume_utf16_byte(offset, byte)
                    {
                        self.consume_code_unit(index, unit_offset, unit)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn consume_code_unit(
        &mut self,
        index: usize,
        unit_offset: u64,
        unit: u16,
    ) -> Result<(), S::Error> {
        let pending_high = self.decoders[index].take_high_surrogate();
        if let Some((high_offset, high)) = pending_high {
            if (0xDC00..=0xDFFF).contains(&unit) {
                let value =
                    0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                let character = char::from_u32(value).expect("valid surrogate pair");
                if is_string_character(character) {
                    self.add_character(index, high_offset, 4, character)?;
                } else {
                    self.finish_detected_candidate(index)?;
                }
                return Ok(());
            }
            self.finish_detected_candidate(index)?;
        }

        if (0xD800..=0xDBFF).contains(&unit) {
            self.decoders[index].set_high_surrogate(unit_offset, unit);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            self.finish_detected_candidate(index)?;
        } else {
            let character = char::from_u32(u32::from(unit)).expect("non-surrogate UTF-16 unit");
            if is_string_character(character) {
                self.add_character(index, unit_offset, 2, character)?;
            } else {
                self.finish_detected_candidate(index)?;
            }
        }
        Ok(())
    }

    fn add_character(
        &mut self,
        index: usize,
        offset: u64,
        width: u64,
        character: char,
    ) -> Result<(), S::Error> {
        let encoding = self.decoders[index].encoding;
        let candidate = &mut self.decoders[index].candidate;
        if candidate.character_count == 0 {
            candidate.start = offset;
        }
        candidate.end = offset + width;
        candidate.character_count += 1;
        if character.is_ascii() {
            candidate.ascii_character_count += 1;
        }

        if let Some(open) = candidate.open.as_mut() {
            if !open.skipped {
                let mut encoded = [0_u8; 4];
                let text = character.encode_utf8(&mut encoded);
                if self.sink.chunk(open.id, text)? == SinkControl::SkipCurrent {
                    open.skipped = true;
                }
            }
            return Ok(());
        }

        candidate.prefix.push(character);
        if candidate.character_count != self.min_length as u64 {
            return Ok(());
        }

        let id = HitId(self.next_hit_id);
        self.next_hit_id += 1;
        let start = HitStart {
            offset: SourceOffset(candidate.start),
            encoding,
        };
        let skipped = self.sink.start(id, start)? == SinkControl::SkipCurrent;
        candidate.open = Some(OpenHit { id, skipped });
        if !skipped && self.sink.chunk(id, &candidate.prefix)? == SinkControl::SkipCurrent {
            candidate.open.as_mut().expect("open hit").skipped = true;
        }
        candidate.prefix.clear();
        Ok(())
    }

    fn finish_detected_candidate(&mut self, index: usize) -> Result<(), S::Error> {
        let suppress = self.is_shifted_artifact(index);
        if !suppress {
            let artifacts = (0..self.decoders.len())
                .filter(|other_index| {
                    *other_index != index
                        && self.decoders[*other_index].encoding != Encoding::ASCII
                        && is_artifact_of(
                            &self.decoders[index].candidate,
                            &self.decoders[*other_index].candidate,
                        )
                })
                .collect::<Vec<_>>();
            for artifact in artifacts {
                self.finish_candidate(artifact, true)?;
            }
        }
        self.finish_candidate(index, suppress)
    }

    fn finish_candidate(&mut self, index: usize, suppress: bool) -> Result<(), S::Error> {
        let open = self.decoders[index].candidate.open;
        if let Some(open) = open {
            if suppress {
                self.sink.abort(open.id)?;
                self.summary.suppressed_candidate_count += 1;
            } else {
                let candidate = &self.decoders[index].candidate;
                self.sink.finish(
                    open.id,
                    HitFinish {
                        source_length: SourceLength(candidate.end - candidate.start),
                        character_count: candidate.character_count,
                    },
                )?;
                self.summary.candidate_count += 1;
                if open.skipped {
                    self.summary.skipped_candidate_count += 1;
                }
            }
        }
        self.decoders[index].candidate.reset();
        Ok(())
    }

    fn is_shifted_artifact(&self, index: usize) -> bool {
        let decoder = &self.decoders[index];
        if decoder.encoding == Encoding::ASCII || decoder.candidate.open.is_none() {
            return false;
        }
        self.decoders
            .iter()
            .enumerate()
            .any(|(other_index, other)| {
                other_index != index
                    && other.encoding != Encoding::ASCII
                    && other.candidate.open.is_some()
                    && is_artifact_of(&other.candidate, &decoder.candidate)
            })
    }

    fn finish_eof(&mut self) -> Result<(), S::Error> {
        let suppress = (0..self.decoders.len())
            .map(|index| self.is_shifted_artifact(index))
            .collect::<Vec<_>>();
        for (index, suppress) in suppress.into_iter().enumerate() {
            self.finish_candidate(index, suppress)?;
        }
        Ok(())
    }

    fn abort_all(&mut self) -> Result<(), S::Error> {
        let mut first_error = None;
        for decoder in &mut self.decoders {
            if let Some(open) = decoder.candidate.open.take() {
                if let Err(error) = self.sink.abort(open.id) {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            decoder.candidate.reset();
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn abort_all_ignoring_errors(&mut self) {
        for decoder in &mut self.decoders {
            if let Some(open) = decoder.candidate.open.take() {
                let _ = self.sink.abort(open.id);
            }
            decoder.candidate.reset();
        }
    }
}

impl Decoder {
    fn consume_utf16_byte(&mut self, offset: u64, byte: u8) -> Option<(u64, u16)> {
        let DecoderKind::Utf16 {
            big_endian,
            alignment,
            first_byte,
            ..
        } = &mut self.kind
        else {
            unreachable!("ASCII decoder has no UTF-16 byte state")
        };
        if offset % 2 == u64::from(*alignment) {
            *first_byte = Some((offset, byte));
            return None;
        }
        let (first_offset, first) = first_byte.take()?;
        debug_assert_eq!(first_offset + 1, offset);
        let unit = if *big_endian {
            u16::from_be_bytes([first, byte])
        } else {
            u16::from_le_bytes([first, byte])
        };
        Some((first_offset, unit))
    }

    fn take_high_surrogate(&mut self) -> Option<(u64, u16)> {
        let DecoderKind::Utf16 { high_surrogate, .. } = &mut self.kind else {
            unreachable!("ASCII decoder has no UTF-16 surrogate state")
        };
        high_surrogate.take()
    }

    fn set_high_surrogate(&mut self, offset: u64, unit: u16) {
        let DecoderKind::Utf16 { high_surrogate, .. } = &mut self.kind else {
            unreachable!("ASCII decoder has no UTF-16 surrogate state")
        };
        *high_surrogate = Some((offset, unit));
    }
}

fn is_string_character(character: char) -> bool {
    !character.is_control() || matches!(character, '\t' | '\n' | '\r')
}

fn is_ascii_string_byte(byte: u8) -> bool {
    (b' '..=b'~').contains(&byte) || matches!(byte, b'\t' | b'\n' | b'\r')
}

fn is_shifted_suffix(other: &Candidate, candidate: &Candidate) -> bool {
    other.ascii_character_count == other.character_count
        && other.start.checked_add(1) == Some(candidate.start)
        && other.end == candidate.end + 1
        && other.character_count == candidate.character_count + 1
}

fn is_non_ascii_shifted_copy(other: &Candidate, candidate: &Candidate) -> bool {
    other.ascii_character_count == other.character_count
        && candidate.ascii_character_count != candidate.character_count
        && other.start.abs_diff(candidate.start) == 1
        && other.end.abs_diff(candidate.end) <= 1
        && other.character_count.abs_diff(candidate.character_count) <= 1
}

fn is_artifact_of(other: &Candidate, candidate: &Candidate) -> bool {
    other.open.is_some()
        && candidate.open.is_some()
        && (is_shifted_suffix(other, candidate) || is_non_ascii_shifted_copy(other, candidate))
}
