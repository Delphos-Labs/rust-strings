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
rust-strings = "0.8.0"
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

# You can set encoding if you need (default is 'ascii', options are 'utf-8', 'utf-16le', 'utf-16be')
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

### Encodings

- `ASCII` reads printable ASCII bytes and tab.
- `UTF8` reads well-formed UTF-8 as defined by Unicode Table 3-7 and RFC 3629.
  Overlong forms, surrogate code points (`ED A0..BF ..`), values above U+10FFFF,
  the bytes `C0`, `C1`, and `F5..FF`, and continuation bytes without a lead are
  ill-formed. An ill-formed byte ends the current string and is never decoded.
  The decoder then resynchronizes as the Unicode maximal-subpart practice does
  (Unicode §3.9), so a well-formed sequence that starts inside the bad bytes is
  still found. Parse it from `"utf8"` or `"utf-8"`.
- `UTF16LE` and `UTF16BE` read UTF-16 at both byte alignments and pair
  surrogates. An unpaired surrogate ends the string.

Offsets and source lengths are byte offsets into the input for every encoding.

### String characters

A string holds printable characters and tab. Other control characters end it,
including line feed and carriage return, as in GNU `strings` without
`--include-all-whitespace`. The Unicode line and paragraph separators (U+2028,
U+2029) end a UTF-8 or UTF-16 string too, so each line of embedded text is its
own hit.

### Ordering

The scanner processes encodings in option order. UTF-16 alignment zero runs
before alignment one. It assigns hit IDs when candidates reach the minimum.

Chunks follow their start immediately. At a shared boundary, the scanner
finishes shadowed readings before it finishes the reading that shadows them.
At EOF, it closes decoders in the same order.

### Shadowed readings

When two decoders read nearly the same bytes, the losing reading still
finishes, with `HitFinish::shadowed` set to `true`. A shadowed hit ends at its
own end, or where its winner ends if the winner finishes first, and its
decoder continues with a fresh candidate. A shadowed hit is not evidence for
later decisions about its decoder lane, and `ScanSummary::candidate_count`
does not count it. `strings`, `dump_strings`, the Python bindings, and the
command line tool drop shadowed hits; a `StringSink` passed to `scan` receives
them.

Only UTF-16 readings are shadowed. The rules are:

- ASCII shadows a UTF-16 view with the same byte range, within one alignment
  byte, that contains non-ASCII scalars. This marks UTF-16 garbage read from
  normal ASCII. Null-interleaved or high-byte UTF-16 text is not shadowed. Real
  UTF-16 text whose bytes are all printable ASCII, such as some CJK strings, is
  shadowed too.
- An all-ASCII UTF-16 hit shadows its one-byte shifted suffix and non-ASCII
  shifted copy. Where this rule applies, it decides the pair.
- UTF-16LE and UTF-16BE readings at opposite byte alignments overlap. Text
  whose characters share one high byte, such as ASCII, Greek, or Cyrillic,
  reads as nearly the same text in both. Two such readings pair when one lies
  inside the other's span widened by one byte at each end. When paired
  readings start one byte apart, the scanner prefers the lane that starts at
  source offset zero or continues one NUL code unit after its previous
  unshadowed hit; otherwise, and when neither or both lanes have that
  evidence, it prefers UTF-16LE, the common case.
- Other overlapping readings, such as a same-endian reading of non-ASCII text
  at the other alignment, stay unshadowed because the bytes do not prove which
  interpretation is intentional.

UTF-8 takes no part in these rules. A UTF-8 string with a non-ASCII character
is reported beside the ASCII pieces of the same bytes. A UTF-8 candidate whose
characters are all ASCII is the ASCII decoder's hit at the same offset, so when
the ASCII decoder runs, the scanner aborts it. That abort and the aborts after
a reader or counter failure are the only aborts.

If any sink callback fails, scanning stops without another sink callback.

## Contributing
Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

Please make sure to update tests as appropriate.

## License
[MIT](https://choosealicense.com/licenses/mit/)
