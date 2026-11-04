use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::num::NonZeroU64;

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
    /// The complete decoded text length in UTF-8 bytes.
    pub decoded_utf8_length: u64,
    /// Whether another reading of the same or nearly the same source bytes won.
    ///
    /// The winner can itself be shadowed by a third reading, but every such chain ends at an
    /// unshadowed hit finished in the same scan. A shadowed hit ends when its winner ends, so it
    /// holds only the characters decoded until then, and its decoder continues with a fresh
    /// candidate.
    pub shadowed: bool,
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
/// While callbacks succeed, `start` is followed by one `finish` or `abort`
/// with the same [`HitId`]. A hit is provisional until `finish` succeeds.
/// The scanner aborts a hit only when it is an all-ASCII UTF-8 copy of the
/// ASCII decoder's hit, or when the reader or a counter fails.
/// After any callback returns an error, the scanner makes no more callbacks.
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
    min_length: NonZeroU64,
    encodings: Vec<Encoding>,
}

impl ScanOptions {
    /// Validates a nonzero minimum and at least one selected encoding.
    pub fn new(
        min_length: usize,
        encodings: impl IntoIterator<Item = Encoding>,
    ) -> Result<Self, ScanOptionsError> {
        let min_length =
            u64::try_from(min_length).map_err(|_| ScanOptionsError::MinimumLengthTooLarge)?;
        let min_length = NonZeroU64::new(min_length).ok_or(ScanOptionsError::ZeroMinimumLength)?;
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
    pub fn min_length(&self) -> u64 {
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
    /// The minimum candidate length does not fit the `u64` domain.
    MinimumLengthTooLarge,
    /// The encoding list was empty.
    NoEncodings,
}

impl fmt::Display for ScanOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMinimumLength => f.write_str("minimum length must be greater than zero"),
            Self::MinimumLengthTooLarge => f.write_str("minimum length exceeds u64"),
            Self::NoEncodings => f.write_str("at least one encoding must be selected"),
        }
    }
}

impl Error for ScanOptionsError {}

/// A failure from the caller-owned reader or sink.
#[derive(Debug)]
pub enum ScanError<E> {
    /// A configuration could not produce valid scanner options.
    Config(ScanOptionsError),
    /// The reader failed.
    Reader(io::Error),
    /// The sink failed.
    Sink(E),
    /// A source or result counter exceeded `u64`.
    Overflow(OverflowError),
}

impl<E: fmt::Display> fmt::Display for ScanError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "configuration error: {error}"),
            Self::Reader(error) => write!(f, "reader error: {error}"),
            Self::Sink(error) => write!(f, "sink error: {error}"),
            Self::Overflow(error) => write!(f, "counter overflow: {error}"),
        }
    }
}

impl<E> Error for ScanError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Reader(error) => Some(error),
            Self::Sink(error) => Some(error),
            Self::Overflow(error) => Some(error),
        }
    }
}

/// Identifies a scanner counter that exceeded `u64`.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum OverflowError {
    BytesRead,
    HitId,
    SourceEnd,
    CharacterCount,
    DecodedUtf8Length,
    CandidateCount,
    SkippedCandidateCount,
    ShadowedCandidateCount,
    DuplicateCandidateCount,
}

impl fmt::Display for OverflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl Error for OverflowError {}

/// Counts produced by one complete scan.
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct ScanSummary {
    /// The number of source bytes consumed.
    pub bytes_read: u64,
    /// The unshadowed candidates completed through `StringSink::finish`.
    pub candidate_count: u64,
    /// The completed candidates, shadowed or not, for which the sink skipped text.
    pub skipped_candidate_count: u64,
    /// The candidates completed through `StringSink::finish` with `HitFinish::shadowed` set.
    pub shadowed_candidate_count: u64,
    /// The all-ASCII UTF-8 candidates discarded through `StringSink::abort` because the ASCII
    /// decoder reports the same text at the same offset.
    pub duplicate_candidate_count: u64,
}

#[derive(Debug)]
struct Candidate {
    start: u64,
    end: u64,
    character_count: u64,
    ascii_character_count: u64,
    decoded_utf8_length: u64,
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
            decoded_utf8_length: 0,
            prefix: String::new(),
            open: None,
        }
    }

    fn reset(&mut self) {
        self.start = 0;
        self.end = 0;
        self.character_count = 0;
        self.ascii_character_count = 0;
        self.decoded_utf8_length = 0;
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
    Utf8 {
        pending: Option<Utf8Sequence>,
    },
    Utf16 {
        big_endian: bool,
        alignment: u8,
        first_byte: Option<(u64, u8)>,
        high_surrogate: Option<(u64, u16)>,
    },
}

/// An incomplete UTF-8 sequence whose bytes so far are a well-formed prefix.
#[derive(Debug, Copy, Clone)]
struct Utf8Sequence {
    start: u64,
    value: u32,
    remaining: u8,
    /// The inclusive range for the next continuation byte, from Unicode Table 3-7.
    next: (u8, u8),
}

/// What one byte completes in the UTF-8 decoder.
#[derive(Debug, Copy, Clone)]
enum Utf8Step {
    /// The byte extends an incomplete sequence.
    Pending,
    /// The byte completes a scalar value that starts at `offset`.
    Scalar {
        offset: u64,
        width: u64,
        character: char,
    },
    /// The byte cannot start a well-formed sequence.
    IllFormed,
}

#[derive(Debug)]
struct Decoder {
    encoding: Encoding,
    kind: DecoderKind,
    candidate: Candidate,
}

struct Scanner<'a, S> {
    sink: &'a mut S,
    min_length: u64,
    decoders: Vec<Decoder>,
    next_hit_id: u64,
    summary: ScanSummary,
    /// Whether the ASCII decoder runs, so an all-ASCII UTF-8 candidate duplicates its hit.
    ascii_selected: bool,
    last_ascii: Option<CandidateRange>,
    last_utf16: Vec<Option<CandidateRange>>,
}

#[derive(Debug, Copy, Clone)]
struct CandidateRange {
    start: u64,
    end: u64,
    character_count: u64,
    ascii_character_count: u64,
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
                return Err(scanner.terminate(ScanError::Reader(error)));
            }
        };

        for byte in &buffer[..read] {
            let offset = scanner.summary.bytes_read;
            if let Err(error) = scanner.consume(offset, *byte) {
                return Err(scanner.terminate(error));
            }
            if let Err(error) =
                checked_increment(&mut scanner.summary.bytes_read, OverflowError::BytesRead)
            {
                return Err(scanner.terminate(ScanError::Overflow(error)));
            }
        }
    }

    if let Err(error) = scanner.finish_eof() {
        return Err(scanner.terminate(error));
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
                Encoding::UTF8 => decoders.push(Decoder {
                    encoding: *encoding,
                    kind: DecoderKind::Utf8 { pending: None },
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
        let last_utf16 = vec![None; decoders.len()];
        Self {
            sink,
            min_length: options.min_length(),
            decoders,
            next_hit_id: 0,
            summary: ScanSummary::default(),
            ascii_selected: options.encodings.contains(&Encoding::ASCII),
            last_ascii: None,
            last_utf16,
        }
    }

    fn consume(&mut self, offset: u64, byte: u8) -> Result<(), ScanError<S::Error>> {
        for index in 0..self.decoders.len() {
            match self.decoders[index].kind {
                DecoderKind::Ascii => {
                    if is_ascii_string_byte(byte) {
                        self.add_character(index, offset, 1, char::from(byte))?;
                    } else {
                        self.finish_detected_candidate(index)?;
                    }
                }
                DecoderKind::Utf8 { .. } => {
                    if self.decoders[index].interrupt_utf8_sequence(byte) {
                        // The incomplete sequence is a maximal subpart of an ill-formed
                        // sequence; this byte may start the next well-formed one.
                        self.finish_detected_candidate(index)?;
                    }
                    match self.decoders[index].consume_utf8_byte(offset, byte) {
                        Utf8Step::Pending => {}
                        Utf8Step::Scalar {
                            offset,
                            width,
                            character,
                        } if is_string_character(character) => {
                            self.add_character(index, offset, width, character)?;
                        }
                        Utf8Step::Scalar { .. } | Utf8Step::IllFormed => {
                            self.finish_detected_candidate(index)?;
                        }
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
    ) -> Result<(), ScanError<S::Error>> {
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
    ) -> Result<(), ScanError<S::Error>> {
        let encoding = self.decoders[index].encoding;
        let candidate = &mut self.decoders[index].candidate;
        if candidate.character_count == 0 {
            candidate.start = offset;
        }
        candidate.end = offset
            .checked_add(width)
            .ok_or(ScanError::Overflow(OverflowError::SourceEnd))?;
        checked_increment(
            &mut candidate.character_count,
            OverflowError::CharacterCount,
        )
        .map_err(ScanError::Overflow)?;
        candidate.decoded_utf8_length = candidate
            .decoded_utf8_length
            .checked_add(character.len_utf8() as u64)
            .ok_or(ScanError::Overflow(OverflowError::DecodedUtf8Length))?;
        if character.is_ascii() {
            checked_increment(
                &mut candidate.ascii_character_count,
                OverflowError::CharacterCount,
            )
            .map_err(ScanError::Overflow)?;
        }

        if let Some(open) = candidate.open.as_mut() {
            if !open.skipped {
                let mut encoded = [0_u8; 4];
                let text = character.encode_utf8(&mut encoded);
                if self.sink.chunk(open.id, text).map_err(ScanError::Sink)?
                    == SinkControl::SkipCurrent
                {
                    open.skipped = true;
                }
            }
            return Ok(());
        }

        candidate.prefix.push(character);
        if candidate.character_count != self.min_length {
            return Ok(());
        }

        let id = HitId(self.next_hit_id);
        self.next_hit_id = self
            .next_hit_id
            .checked_add(1)
            .ok_or(ScanError::Overflow(OverflowError::HitId))?;
        let start = HitStart {
            offset: SourceOffset(candidate.start),
            encoding,
        };
        let skipped =
            self.sink.start(id, start).map_err(ScanError::Sink)? == SinkControl::SkipCurrent;
        candidate.open = Some(OpenHit { id, skipped });
        if !skipped
            && self
                .sink
                .chunk(id, &candidate.prefix)
                .map_err(ScanError::Sink)?
                == SinkControl::SkipCurrent
        {
            candidate.open.as_mut().expect("open hit").skipped = true;
        }
        candidate.prefix.clear();
        Ok(())
    }

    fn finish_detected_candidate(&mut self, index: usize) -> Result<(), ScanError<S::Error>> {
        let shadowed = self.is_shadowed(index);
        if !shadowed {
            let losers = (0..self.decoders.len())
                .filter(|other_index| *other_index != index && self.shadows(index, *other_index))
                .collect::<Vec<_>>();
            for loser in losers {
                self.finish_candidate(loser, true)?;
            }
        }
        self.finish_candidate(index, shadowed)
    }

    fn finish_candidate(
        &mut self,
        index: usize,
        shadowed: bool,
    ) -> Result<(), ScanError<S::Error>> {
        let Some(open) = self.decoders[index].candidate.open else {
            self.decoders[index].candidate.reset();
            return Ok(());
        };
        let encoding = self.decoders[index].encoding;
        let candidate = &self.decoders[index].candidate;
        if encoding == Encoding::UTF8
            && self.ascii_selected
            && candidate.ascii_character_count == candidate.character_count
        {
            // The ASCII decoder accepts the same bytes and ends at the same bytes, so it
            // reports this text at this offset.
            self.sink.abort(open.id).map_err(ScanError::Sink)?;
            self.decoders[index].candidate.reset();
            return checked_increment(
                &mut self.summary.duplicate_candidate_count,
                OverflowError::DuplicateCandidateCount,
            )
            .map_err(ScanError::Overflow);
        }

        let source_length = candidate
            .end
            .checked_sub(candidate.start)
            .ok_or(ScanError::Overflow(OverflowError::SourceEnd))?;
        let range = CandidateRange::from(candidate);
        self.sink
            .finish(
                open.id,
                HitFinish {
                    source_length: SourceLength(source_length),
                    character_count: candidate.character_count,
                    decoded_utf8_length: candidate.decoded_utf8_length,
                    shadowed,
                },
            )
            .map_err(ScanError::Sink)?;
        self.decoders[index].candidate.reset();
        if shadowed {
            checked_increment(
                &mut self.summary.shadowed_candidate_count,
                OverflowError::ShadowedCandidateCount,
            )
            .map_err(ScanError::Overflow)?;
        } else {
            // Only a winning reading is evidence for the next shadow decision.
            match encoding {
                Encoding::ASCII => self.last_ascii = Some(range),
                Encoding::UTF16LE | Encoding::UTF16BE => self.last_utf16[index] = Some(range),
                Encoding::UTF8 => {}
            }
            checked_increment(
                &mut self.summary.candidate_count,
                OverflowError::CandidateCount,
            )
            .map_err(ScanError::Overflow)?;
        }
        if open.skipped {
            checked_increment(
                &mut self.summary.skipped_candidate_count,
                OverflowError::SkippedCandidateCount,
            )
            .map_err(ScanError::Overflow)?;
        }
        Ok(())
    }

    fn is_shadowed(&self, index: usize) -> bool {
        let decoder = &self.decoders[index];
        if !is_utf16(decoder.encoding) || decoder.candidate.open.is_none() {
            return false;
        }
        let candidate = CandidateRange::from(&decoder.candidate);
        self.last_ascii
            .is_some_and(|ascii| ascii_shadows(ascii, candidate))
            || self
                .decoders
                .iter()
                .enumerate()
                .any(|(other_index, other)| {
                    other_index != index
                        && other.candidate.open.is_some()
                        && self.shadows(other_index, index)
                })
    }

    /// Whether the open candidate at `winner_index` shadows the open candidate at
    /// `candidate_index`. Only UTF-16 readings are ever shadowed, by ASCII or UTF-16 winners.
    fn shadows(&self, winner_index: usize, candidate_index: usize) -> bool {
        let winner = &self.decoders[winner_index];
        let candidate = &self.decoders[candidate_index];
        if winner.candidate.open.is_none()
            || candidate.candidate.open.is_none()
            || !is_utf16(candidate.encoding)
        {
            return false;
        }
        let winner_range = CandidateRange::from(&winner.candidate);
        let candidate_range = CandidateRange::from(&candidate.candidate);
        match winner.encoding {
            Encoding::ASCII => ascii_shadows(winner_range, candidate_range),
            Encoding::UTF16LE | Encoding::UTF16BE => {
                utf16_shadows_shifted(winner_range, candidate_range)
                    || self.prefers_exact_ascii_copy(
                        winner_index,
                        winner_range,
                        candidate_index,
                        candidate_range,
                    )
            }
            Encoding::UTF8 => false,
        }
    }

    fn prefers_exact_ascii_copy(
        &self,
        winner_index: usize,
        winner_range: CandidateRange,
        candidate_index: usize,
        candidate_range: CandidateRange,
    ) -> bool {
        let winner_encoding = self.decoders[winner_index].encoding;
        if !is_exact_opposite_endian_ascii_copy(
            winner_encoding,
            winner_range,
            self.decoders[candidate_index].encoding,
            candidate_range,
        ) {
            return false;
        }

        // Exact copies are byte-ambiguous. Prefer a lane established by the
        // source boundary or an adjacent terminated hit.
        let winner_evidence = winner_range.start == 0
            || follows_utf16_neighbor(self.last_utf16[winner_index], winner_range);
        let candidate_evidence = candidate_range.start == 0
            || follows_utf16_neighbor(self.last_utf16[candidate_index], candidate_range);
        if winner_evidence != candidate_evidence {
            return winner_evidence;
        }
        // Without deciding evidence, read UTF-16 as little-endian, as FLOSS and YARA `wide` do.
        // The big-endian copy is still reported, shadowed.
        winner_encoding == Encoding::UTF16LE
    }

    fn finish_eof(&mut self) -> Result<(), ScanError<S::Error>> {
        for index in 0..self.decoders.len() {
            self.finish_detected_candidate(index)?;
        }
        Ok(())
    }

    fn abort_all(&mut self) -> Result<(), ScanError<S::Error>> {
        for decoder in &mut self.decoders {
            if let Some(open) = decoder.candidate.open.take() {
                self.sink.abort(open.id).map_err(ScanError::Sink)?;
            }
            decoder.candidate.reset();
        }
        Ok(())
    }

    fn terminate(&mut self, error: ScanError<S::Error>) -> ScanError<S::Error> {
        match error {
            ScanError::Sink(_) => error,
            error => match self.abort_all() {
                Ok(()) => error,
                Err(sink_error) => sink_error,
            },
        }
    }
}

impl From<&Candidate> for CandidateRange {
    fn from(candidate: &Candidate) -> Self {
        Self {
            start: candidate.start,
            end: candidate.end,
            character_count: candidate.character_count,
            ascii_character_count: candidate.ascii_character_count,
        }
    }
}

fn checked_increment(value: &mut u64, error: OverflowError) -> Result<(), OverflowError> {
    match value.checked_add(1) {
        Some(next) => {
            *value = next;
            Ok(())
        }
        None => Err(error),
    }
}

fn is_utf16(encoding: Encoding) -> bool {
    matches!(encoding, Encoding::UTF16LE | Encoding::UTF16BE)
}

fn ascii_shadows(ascii: CandidateRange, utf16: CandidateRange) -> bool {
    utf16.ascii_character_count != utf16.character_count
        && ascii.start.abs_diff(utf16.start) <= 1
        && ascii.end.abs_diff(utf16.end) <= 1
}

fn utf16_shadows_shifted(winner: CandidateRange, candidate: CandidateRange) -> bool {
    if winner.ascii_character_count != winner.character_count {
        return false;
    }
    let shifted_suffix = winner.start.checked_add(1) == Some(candidate.start)
        && candidate.end.checked_add(1) == Some(winner.end)
        && candidate.character_count.checked_add(1) == Some(winner.character_count);
    let non_ascii_copy = candidate.ascii_character_count != candidate.character_count
        && winner.start.abs_diff(candidate.start) == 1
        && winner.end.abs_diff(candidate.end) <= 1
        && winner.character_count.abs_diff(candidate.character_count) <= 1;
    shifted_suffix || non_ascii_copy
}

fn is_exact_opposite_endian_ascii_copy(
    winner_encoding: Encoding,
    winner: CandidateRange,
    candidate_encoding: Encoding,
    candidate: CandidateRange,
) -> bool {
    let adjacent_opposite_endian = match (winner_encoding, candidate_encoding) {
        (Encoding::UTF16LE, Encoding::UTF16BE) => {
            candidate.start.checked_add(1) == Some(winner.start)
                && candidate.end.checked_add(1) == Some(winner.end)
        }
        (Encoding::UTF16BE, Encoding::UTF16LE) => {
            winner.start.checked_add(1) == Some(candidate.start)
                && winner.end.checked_add(1) == Some(candidate.end)
        }
        _ => false,
    };
    adjacent_opposite_endian
        && winner.ascii_character_count == winner.character_count
        && candidate.ascii_character_count == candidate.character_count
        && winner.character_count == candidate.character_count
}

fn follows_utf16_neighbor(previous: Option<CandidateRange>, candidate: CandidateRange) -> bool {
    // One UTF-16 NUL code unit separates neighboring candidates.
    previous.and_then(|range| range.end.checked_add(2)) == Some(candidate.start)
}

impl Decoder {
    /// Ends an incomplete UTF-8 sequence that `byte` cannot continue, and reports whether it
    /// did. The caller then passes `byte` to [`Self::consume_utf8_byte`] as a new sequence.
    fn interrupt_utf8_sequence(&mut self, byte: u8) -> bool {
        let DecoderKind::Utf8 { pending } = &mut self.kind else {
            unreachable!("only the UTF-8 decoder has UTF-8 sequence state")
        };
        match pending {
            Some(sequence) if !(sequence.next.0..=sequence.next.1).contains(&byte) => {
                *pending = None;
                true
            }
            _ => false,
        }
    }

    /// Decodes well-formed UTF-8 as defined by Unicode Table 3-7 and RFC 3629, which excludes
    /// overlong forms, surrogate code points, and values above U+10FFFF.
    fn consume_utf8_byte(&mut self, offset: u64, byte: u8) -> Utf8Step {
        let DecoderKind::Utf8 { pending } = &mut self.kind else {
            unreachable!("only the UTF-8 decoder has UTF-8 sequence state")
        };
        if let Some(sequence) = pending.as_mut() {
            debug_assert!((sequence.next.0..=sequence.next.1).contains(&byte));
            sequence.value = (sequence.value << 6) | u32::from(byte & 0x3f);
            sequence.remaining -= 1;
            sequence.next = (0x80, 0xbf);
            if sequence.remaining > 0 {
                return Utf8Step::Pending;
            }
            let start = sequence.start;
            let value = sequence.value;
            *pending = None;
            return Utf8Step::Scalar {
                offset: start,
                width: offset - start + 1,
                character: char::from_u32(value).expect("Table 3-7 admits only scalar values"),
            };
        }
        let (value, remaining, next) = match byte {
            0x00..=0x7f => {
                return Utf8Step::Scalar {
                    offset,
                    width: 1,
                    character: char::from(byte),
                }
            }
            0xc2..=0xdf => (byte & 0x1f, 1, (0x80, 0xbf)),
            0xe0 => (byte & 0x0f, 2, (0xa0, 0xbf)),
            0xe1..=0xec | 0xee..=0xef => (byte & 0x0f, 2, (0x80, 0xbf)),
            0xed => (byte & 0x0f, 2, (0x80, 0x9f)),
            0xf0 => (byte & 0x07, 3, (0x90, 0xbf)),
            0xf1..=0xf3 => (byte & 0x07, 3, (0x80, 0xbf)),
            0xf4 => (byte & 0x07, 3, (0x80, 0x8f)),
            // C0, C1, and F5..FF never occur; 80..BF cannot start a sequence.
            _ => return Utf8Step::IllFormed,
        };
        *pending = Some(Utf8Sequence {
            start: offset,
            value: u32::from(value),
            remaining,
            next,
        });
        Utf8Step::Pending
    }

    fn consume_utf16_byte(&mut self, offset: u64, byte: u8) -> Option<(u64, u16)> {
        let DecoderKind::Utf16 {
            big_endian,
            alignment,
            first_byte,
            ..
        } = &mut self.kind
        else {
            unreachable!("only UTF-16 decoders have UTF-16 byte state")
        };
        if offset % 2 == u64::from(*alignment) {
            *first_byte = Some((offset, byte));
            return None;
        }
        let (first_offset, first) = first_byte.take()?;
        debug_assert_eq!(first_offset.checked_add(1), Some(offset));
        let unit = if *big_endian {
            u16::from_be_bytes([first, byte])
        } else {
            u16::from_le_bytes([first, byte])
        };
        Some((first_offset, unit))
    }

    fn take_high_surrogate(&mut self) -> Option<(u64, u16)> {
        let DecoderKind::Utf16 { high_surrogate, .. } = &mut self.kind else {
            unreachable!("only UTF-16 decoders have UTF-16 surrogate state")
        };
        high_surrogate.take()
    }

    fn set_high_surrogate(&mut self, offset: u64, unit: u16) {
        let DecoderKind::Utf16 { high_surrogate, .. } = &mut self.kind else {
            unreachable!("only UTF-16 decoders have UTF-16 surrogate state")
        };
        *high_surrogate = Some((offset, unit));
    }
}

/// Whether `character` can be part of a decoded string: any non-control character except the
/// Unicode line and paragraph separators, plus tab. Every line break ends the string, as in GNU
/// `strings` without `--include-all-whitespace`, so each line of embedded text is its own hit
/// with its own source offset, and no hit contains a character that a line splitter breaks on.
fn is_string_character(character: char) -> bool {
    (!character.is_control() && !matches!(character, '\u{2028}' | '\u{2029}')) || character == '\t'
}

/// The ASCII form of [`is_string_character`]: printable ASCII plus tab.
fn is_ascii_string_byte(byte: u8) -> bool {
    (b' '..=b'~').contains(&byte) || byte == b'\t'
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    struct NoopSink;

    impl StringSink for NoopSink {
        type Error = Infallible;

        fn start(&mut self, _id: HitId, _hit: HitStart) -> Result<SinkControl, Self::Error> {
            Ok(SinkControl::Continue)
        }

        fn chunk(&mut self, _id: HitId, _text: &str) -> Result<SinkControl, Self::Error> {
            Ok(SinkControl::Continue)
        }

        fn finish(&mut self, _id: HitId, _hit: HitFinish) -> Result<(), Self::Error> {
            Ok(())
        }

        fn abort(&mut self, _id: HitId) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn hit_id_overflow_is_typed_and_happens_before_start() {
        let options = ScanOptions::new(1, [Encoding::ASCII]).unwrap();
        let mut sink = NoopSink;
        let mut scanner = Scanner::new(&options, &mut sink);
        scanner.next_hit_id = u64::MAX;

        assert!(matches!(
            scanner.consume(0, b'A'),
            Err(ScanError::Overflow(OverflowError::HitId))
        ));
        assert!(scanner.decoders[0].candidate.open.is_none());
    }

    #[test]
    fn decoded_length_overflow_is_typed() {
        let options = ScanOptions::new(2, [Encoding::ASCII]).unwrap();
        let mut sink = NoopSink;
        let mut scanner = Scanner::new(&options, &mut sink);
        scanner.decoders[0].candidate.decoded_utf8_length = u64::MAX;

        assert!(matches!(
            scanner.add_character(0, 0, 1, 'A'),
            Err(ScanError::Overflow(OverflowError::DecodedUtf8Length))
        ));
    }

    #[test]
    fn checked_increment_reports_its_counter() {
        let mut value = u64::MAX;
        assert_eq!(
            checked_increment(&mut value, OverflowError::CandidateCount),
            Err(OverflowError::CandidateCount)
        );
    }

    #[test]
    fn completed_candidate_counter_overflow_is_typed() {
        let options = ScanOptions::new(1, [Encoding::ASCII]).unwrap();
        let mut sink = NoopSink;
        let mut scanner = Scanner::new(&options, &mut sink);
        scanner.summary.candidate_count = u64::MAX;
        scanner.consume(0, b'A').unwrap();

        assert!(matches!(
            scanner.consume(1, 0),
            Err(ScanError::Overflow(OverflowError::CandidateCount))
        ));
        assert!(scanner.decoders[0].candidate.open.is_none());
    }
}
