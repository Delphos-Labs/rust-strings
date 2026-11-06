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

    /// Consumes the byte at `offset` and returns a string when the byte
    /// completes a padding run.
    pub(crate) fn consume(&mut self, offset: u64, byte: u8, min_length: u64) -> Option<XorHit> {
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
            return None;
        }
        let key = NonZeroU8::new(self.run_byte)?;
        self.read_back(key, min_length)
    }

    fn read_back(&self, key: NonZeroU8, min_length: u64) -> Option<XorHit> {
        let earliest = self
            .run_start
            .saturating_sub(MAX_STRING_BYTES)
            .max(self.boundary);
        let mut start = self.run_start;
        while start > earliest && is_ascii_string_byte(self.decoded(start - 1, key)) {
            start -= 1;
        }
        if self.run_start - start < min_length {
            return None;
        }
        let text = (start..self.run_start)
            .map(|offset| char::from(self.decoded(offset, key)))
            .collect::<String>();
        has_letter_run(&text).then_some(XorHit { start, key, text })
    }

    fn decoded(&self, offset: u64, key: NonZeroU8) -> u8 {
        self.window[window_index(offset)] ^ key.get()
    }
}

fn window_index(offset: u64) -> usize {
    (offset % WINDOW as u64) as usize
}

fn has_letter_run(text: &str) -> bool {
    let mut run = 0;
    for byte in text.bytes() {
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
