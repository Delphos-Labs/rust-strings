//! ASCII strings XORed with one byte, found through their encoded zero padding.
//!
//! XOR with a key leaves zero bytes equal to the key, so a NUL-terminated or
//! zero-padded string encoded with one byte ends in a run of the key byte. The
//! decoder treats each long run of one non-zero byte as a key and decodes the
//! bytes just before the run.

use std::num::NonZeroU8;

use crate::scanner::is_ascii_string_byte;

/// The run length that marks encoded zero padding. Shorter runs of one byte
/// are common in ordinary code and data.
const MIN_PADDING_RUN: u64 = 8;
/// The most bytes read back from a padding run.
const MAX_STRING_BYTES: u64 = 512;
/// Decoded text must contain this many ASCII letters in a row. Code bytes
/// before instruction padding, such as `0xCC` or `0x90` runs, decode to
/// printable bytes often but rarely to a word.
const MIN_LETTER_RUN: usize = 3;
/// The window holds every byte a read-back can reach: the string bytes before
/// the run and the run bytes seen so far.
const WINDOW: usize = (MAX_STRING_BYTES + MIN_PADDING_RUN) as usize;

/// A decoded string and the key its padding exposed.
pub(crate) struct XorHit {
    pub(crate) start: u64,
    pub(crate) key: NonZeroU8,
    pub(crate) text: String,
}

/// Streaming state for padded one-byte XOR strings, in fixed memory.
pub(crate) struct PaddedXor {
    window: [u8; WINDOW],
    run_byte: u8,
    run_start: u64,
    run_length: u64,
    /// The end of the last padding run before the current run. A read-back
    /// stops there, so the previous slot's padding never prefixes a string.
    boundary: u64,
}

impl PaddedXor {
    pub(crate) fn new() -> Self {
        Self {
            window: [0; WINDOW],
            run_byte: 0,
            run_start: 0,
            run_length: 0,
            boundary: 0,
        }
    }

    /// Consumes the byte at `offset`. When the byte completes a padding run,
    /// returns the strings before the run, one per line, in source order.
    pub(crate) fn consume(&mut self, offset: u64, byte: u8, min_length: u64) -> Vec<XorHit> {
        self.window[window_index(offset)] = byte;
        if self.run_length > 0 && byte == self.run_byte {
            self.run_length = self.run_length.saturating_add(1);
        } else {
            if self.run_length >= MIN_PADDING_RUN {
                self.boundary = offset;
            }
            self.run_byte = byte;
            self.run_start = offset;
            self.run_length = 1;
        }
        if self.run_length != MIN_PADDING_RUN {
            return Vec::new();
        }
        match NonZeroU8::new(self.run_byte) {
            Some(key) => self.read_back(key, min_length),
            None => Vec::new(),
        }
    }

    /// Decodes back from the run through string bytes and line breaks. A line
    /// break ends a string, as in every other decoder, so each line is its own
    /// hit. One letter run anywhere in the decoded text confirms the key for
    /// every line.
    fn read_back(&self, key: NonZeroU8, min_length: u64) -> Vec<XorHit> {
        let earliest = self
            .run_start
            .saturating_sub(MAX_STRING_BYTES)
            .max(self.boundary);
        let mut start = self.run_start;
        while start > earliest && {
            let byte = self.decoded(start - 1, key);
            is_ascii_string_byte(byte) || is_line_break(byte)
        } {
            start -= 1;
        }
        if self.run_start - start < min_length
            || !has_letter_run((start..self.run_start).map(|offset| self.decoded(offset, key)))
        {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut line_start = start;
        for offset in start..=self.run_start {
            if offset == self.run_start || is_line_break(self.decoded(offset, key)) {
                if offset - line_start >= min_length {
                    let text = (line_start..offset)
                        .map(|offset| char::from(self.decoded(offset, key)))
                        .collect();
                    hits.push(XorHit {
                        start: line_start,
                        key,
                        text,
                    });
                }
                line_start = offset + 1;
            }
        }
        hits
    }

    fn decoded(&self, offset: u64, key: NonZeroU8) -> u8 {
        self.window[window_index(offset)] ^ key.get()
    }
}

fn window_index(offset: u64) -> usize {
    (offset % WINDOW as u64) as usize
}

fn is_line_break(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r')
}

fn has_letter_run(bytes: impl Iterator<Item = u8>) -> bool {
    let mut run = 0;
    for byte in bytes {
        if byte.is_ascii_alphabetic() {
            run += 1;
            if run == MIN_LETTER_RUN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}
