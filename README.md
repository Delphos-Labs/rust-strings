# rust-strings

[![CI](https://github.com/Delphos-Labs/rust-strings/workflows/Rust%20Lint%20%26%20Test/badge.svg?branch=main)](https://github.com/Delphos-Labs/rust-strings/actions?query=branch=main)
![License](https://img.shields.io/github/license/Delphos-Labs/rust-strings)
![Crates.io](https://img.shields.io/crates/v/rust-strings)
[![PyPI](https://img.shields.io/pypi/v/rust-strings.svg)](https://pypi.org/project/rust-strings)

`rust-strings` is a Rust library for extracting strings from binary data. \
It also have Python bindings.

## Installation

### Python

Use the package manager [pip](https://pip.pypa.io/en/stable/) to install `rust-strings`.

```bash
pip install rust-strings
```

### Rust

`rust-strings` is available on [crates.io](https://crates.io/crates/rust-strings) and can be included in your Cargo enabled project like this:

```bash
[dependencies]
rust-strings = "0.7.0"
```

## Usage

### Python

```python
import rust_strings

# Get typed ASCII hits from a file with a minimum string length.
hits = rust_strings.strings(file_path="/bin/ls", min_length=3)
# hits[0].text, hits[0].source_offset, hits[0].encoding

# You can also set buffer size when reading from file (default is 1mb)
rust_strings.strings(file_path="/bin/ls", min_length=5, buffer_size=1024)

# You can set encoding if you need (default is 'ascii', options are 'utf-16le', 'utf-16be')
rust_strings.strings(file_path=r"C:\Windows\notepad.exe", min_length=5, encodings=["utf-16le"])

# You can set multiple encoding
rust_strings.strings(file_path=r"C:\Windows\notepad.exe", min_length=5, encodings=["ascii", "utf-16le"])

# You can also pass bytes instead of file_path
rust_strings.strings(bytes=b"test\x00\x00", min_length=4, encodings=["ascii"])
# The result contains StringHit objects with text, offsets, encoding, and lengths.

# You can also dump to json file
rust_strings.dump_strings("strings.json", bytes=b"test\x00\x00", min_length=4, encodings=["ascii"])
# `strings.json` content:
# [["test", 0]]
```

### Rust

Full documentation available in [docs.rs](https://docs.rs/rust-strings)

```rust
use rust_strings::{FileConfig, BytesConfig, strings, dump_strings, Encoding};
use std::path::{Path, PathBuf};

let config = FileConfig::new(Path::new("/bin/ls")).with_min_length(5);
let extracted_strings = strings(&config);

// Extract utf16le strings
let config = FileConfig::new(Path::new("C:\\Windows\\notepad.exe"))
    .with_min_length(15)
    .with_encoding(Encoding::UTF16LE);
let extracted_strings = strings(&config);

// Extract ascii and utf16le strings
let config = FileConfig::new(Path::new("C:\\Windows\\notepad.exe"))
    .with_min_length(15)
    .with_encoding(Encoding::ASCII)
    .with_encoding(Encoding::UTF16LE);
let extracted_strings = strings(&config);

let config = BytesConfig::new(b"test\x00".to_vec());
let extracted_strings = strings(&config);
let hit = extracted_strings.unwrap().remove(0);
assert_eq!(hit.text, "test");
assert_eq!(hit.start.offset.get(), 0);

// Dump strings into `strings.json` file.
let config = BytesConfig::new(b"test\x00".to_vec());
dump_strings(&config, PathBuf::from("strings.json"));
```

Use `scan` when the caller must own input and output streaming. Sink callbacks
can overlap, so each callback includes a stable `HitId`. A sink can return
`SkipCurrent` to stop text delivery while the scanner continues to count the
candidate and scan the input.

### Ordering and suppression

The scanner processes encodings in option order. UTF-16 alignment zero runs
before alignment one. It assigns hit IDs when candidates reach the minimum.

Chunks follow their start immediately. At a shared boundary, the scanner
aborts known artifacts before it finishes the preferred hit. At EOF, it closes
decoders in the same order.

ASCII wins when a UTF-16 view has the same byte range, within one alignment
byte, and contains non-ASCII scalars. This removes UTF-16 garbage from normal
ASCII. It does not suppress null-interleaved or high-byte UTF-16 text.

An all-ASCII UTF-16 hit wins over its one-byte shifted suffix and non-ASCII
shifted copy. Other overlapping Unicode hits remain because the bytes do not
prove which interpretation is intentional.

If both overlapping views decode to valid non-ASCII Unicode, the scanner keeps
both in decoder order. Neither view has stronger byte evidence. Retaining both
avoids falsely deleting a valid string based only on alignment.

If any sink callback fails, scanning stops without another sink callback.

## Contributing
Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

Please make sure to update tests as appropriate.

## License
[MIT](https://choosealicense.com/licenses/mit/)
