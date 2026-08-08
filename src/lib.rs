//! # Rust Strings
//!
//! `rust-strings` is a library to extract ascii strings from binary data.
//! It is similar to the command `strings`.
//!
//! ## Examples:
//! ```
//! use rust_strings::{FileConfig, BytesConfig, strings, Encoding};
//! use std::path::Path;
//!
//! let config = FileConfig::new(Path::new("/bin/ls")).with_min_length(5);
//! let extracted_strings = strings(&config);
//!
//! // Extract utf16le strings
//! let config = FileConfig::new(Path::new("C:\\Windows\\notepad.exe"))
//!     .with_min_length(15)
//!     .with_encoding(Encoding::UTF16LE);
//! let extracted_strings = strings(&config);
//!
//! // Extract ascii and utf16le strings
//! let config = FileConfig::new(Path::new("C:\\Windows\\notepad.exe"))
//!     .with_min_length(15)
//!     .with_encoding(Encoding::ASCII)
//!     .with_encoding(Encoding::UTF16LE);
//! let extracted_strings = strings(&config);
//!
//! let config = BytesConfig::new(b"test\x00".to_vec());
//! let extracted_strings = strings(&config);
//! let hit = extracted_strings.unwrap().remove(0);
//! assert_eq!(hit.text, "test");
//! assert_eq!(hit.start.offset.get(), 0);
//! ```

mod encodings;
mod scanner;
mod strings;

pub use encodings::{Encoding, EncodingNotFoundError};
pub use scanner::{
    scan, HitFinish, HitId, HitStart, OverflowError, ScanError, ScanOptions, ScanOptionsError,
    ScanSummary, SinkControl, SourceLength, SourceOffset, StringSink,
};
pub use strings::{dump_strings, strings, BytesConfig, Config, FileConfig, StdinConfig, StringHit};

#[cfg(feature = "python_bindings")]
mod python_bindings;
