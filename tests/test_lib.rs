use rust_strings::{
    dump_strings, scan, strings, BytesConfig, Config, Encoding, FileConfig, ScanError, ScanOptions,
    ScanOptionsError, ScanSummary, StringHit, StringSink,
};
use std::io::{self, Read, Write};
use tempfile::NamedTempFile;

/// Returns every collected hit as `(text, offset, shadowed)`.
fn extracted<T: Config>(config: &T) -> Vec<(String, u64, bool)> {
    strings(config)
        .unwrap()
        .into_iter()
        .map(|hit| (hit.text, hit.start.offset.get(), hit.finish.shadowed))
        .collect()
}

#[test]
fn default_collection_shadows_utf16_views_of_ascii() {
    let config = BytesConfig::new(b"normal ASCII text\0".to_vec());
    let hits: Vec<StringHit> = strings(&config).unwrap();

    let winners = hits
        .iter()
        .filter(|hit| !hit.finish.shadowed)
        .collect::<Vec<_>>();
    assert_eq!(winners.len(), 1, "{hits:?}");
    assert_eq!(winners[0].text, "normal ASCII text");
    assert_eq!(winners[0].start.encoding, Encoding::ASCII);
    assert_eq!(winners[0].start.offset.get(), 0);
    assert_eq!(winners[0].finish.source_length.get(), 17);
    assert_eq!(winners[0].finish.character_count, 17);
    assert_eq!(winners[0].finish.decoded_utf8_length, 17);

    // Little-endian pairs from offset 0 are "no" "rm" "al" " A" "SC" "II" " t" "ex"; the
    // ASCII hit ends at the NUL while "t" waits for its pair, so that view ends at 16. Pairs
    // from offset 1 are "or" "ma" "l " "AS" "CI" "I " "te" "xt".
    let shadowed = hits
        .iter()
        .filter(|hit| hit.finish.shadowed)
        .map(|hit| {
            (
                hit.text.as_str(),
                hit.start.offset.get(),
                hit.start.encoding,
                hit.finish.source_length.get(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shadowed,
        vec![
            (
                "\u{6f6e}\u{6d72}\u{6c61}\u{4120}\u{4353}\u{4949}\u{7420}\u{7865}",
                0,
                Encoding::UTF16LE,
                16
            ),
            (
                "\u{726f}\u{616d}\u{206c}\u{5341}\u{4943}\u{2049}\u{6574}\u{7478}",
                1,
                Encoding::UTF16LE,
                16
            ),
        ]
    );
}

#[test]
fn encoding_names_parse_and_display() {
    for name in ["utf8", "utf-8", "UTF-8"] {
        assert_eq!(name.parse::<Encoding>().unwrap(), Encoding::UTF8, "{name}");
    }
    assert_eq!("ascii".parse::<Encoding>().unwrap(), Encoding::ASCII);
    assert_eq!("utf-16le".parse::<Encoding>().unwrap(), Encoding::UTF16LE);
    assert_eq!("utf-16be".parse::<Encoding>().unwrap(), Encoding::UTF16BE);
    assert!("latin-1".parse::<Encoding>().is_err());
    assert_eq!(Encoding::UTF8.to_string(), "UTF8");
}

#[test]
fn utf8_collection_reports_the_encoding_and_byte_offsets() {
    // "ö" is c3 b6 and "€" is e2 82 ac.
    let config = BytesConfig::new(b"\0J\xc3\xb6rg \xe2\x82\xac\0".to_vec())
        .with_encodings(vec![Encoding::ASCII, Encoding::UTF8]);
    let hits = strings(&config).unwrap();

    let hit = hits
        .iter()
        .find(|hit| hit.start.encoding == Encoding::UTF8)
        .expect("UTF-8 hit");
    assert_eq!(hit.text, "Jörg €");
    assert_eq!(hit.start.offset.get(), 1);
    assert_eq!(hit.finish.source_length.get(), 9);
    assert_eq!(hit.finish.character_count, 6);
}

#[test]
fn default_collection_keeps_odd_aligned_and_non_ascii_utf16() {
    let mut bytes = vec![0xff];
    for unit in "é😀中".encode_utf16().chain([0]) {
        bytes.extend(unit.to_le_bytes());
    }
    let hits = strings(&BytesConfig::new(bytes)).unwrap();

    let hit = hits
        .iter()
        .find(|hit| hit.text == "é😀中" && hit.start.offset.get() == 1)
        .expect("odd-aligned Unicode hit");
    assert_eq!(hit.start.encoding, Encoding::UTF16LE);
    assert_eq!(hit.finish.decoded_utf8_length, 9);
}

#[test]
fn invalid_config_is_a_typed_error() {
    let config = BytesConfig::new(Vec::new()).with_min_length(0);
    assert!(matches!(
        strings(&config),
        Err(ScanError::Config(ScanOptionsError::ZeroMinimumLength))
    ));
}

#[test]
fn test_bytes_config() {
    let config = BytesConfig::new(vec![116, 101, 115, 116, 0, 0]).with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 0, false)], extracted);
}

#[test]
fn test_extract_one_byte() {
    let config = BytesConfig::new(b"t\x00".to_vec())
        .with_min_length(1)
        .with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("t"), 0, false)], extracted);
}

#[test]
fn test_extract_bytes_min_length_1() {
    let config = BytesConfig::new(b"test\x00".to_vec())
        .with_min_length(1)
        .with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 0, false)], extracted);
}

#[test]
fn test_bytes_config_bytes_array() {
    let config = BytesConfig::new(b"test\x00".to_vec()).with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 0, false)], extracted);
}

#[test]
fn test_bytes_config_offset() {
    let config = BytesConfig::new(vec![0, 116, 101, 115, 116]).with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 1, false)], extracted);
}

#[test]
fn test_bytes_config_min_length() {
    let config = BytesConfig::new(vec![116, 101, 115, 116, 0, 0, 116, 101, 115])
        .with_min_length(4)
        .with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 0, false)], extracted);
}

#[test]
fn test_bytes_config_multiple_strings() {
    let config = BytesConfig::new(vec![116, 101, 115, 116, 0, 0, 116, 101, 115])
        .with_min_length(3)
        .with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(
        vec![
            (String::from("test"), 0, false),
            (String::from("tes"), 6, false)
        ],
        extracted
    );
}

#[test]
fn test_file_config() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(b"test\x00").unwrap();

    let path = file.path();
    let config = FileConfig::new(path).with_encoding(Encoding::ASCII);
    let extracted = extracted(&config);
    assert_eq!(vec![(String::from("test"), 0, false)], extracted);
}

#[test]
fn test_utf16le() {
    let config =
        BytesConfig::new(b"t\x00e\x00s\x00t\x00\x00\x00".to_vec()).with_encoding(Encoding::UTF16LE);
    let extracted = extracted(&config);
    // Pairs from offset 1 are 00 65, 00 73, 00 74: the swapped "est", shadowed by "test".
    assert_eq!(
        vec![
            (String::from("test"), 0, false),
            (String::from("\u{6500}\u{7300}\u{7400}"), 1, true)
        ],
        extracted
    );
}

#[test]
fn test_utf16be() {
    let config = BytesConfig::new(b"\x00t\x00e\x00s\x00t\x00\x00\x00".to_vec())
        .with_encoding(Encoding::UTF16BE);
    let extracted = extracted(&config);
    // Pairs from offset 1 are 74 00, 65 00, 73 00, 74 00: the swapped "test", shadowed.
    assert_eq!(
        vec![
            (String::from("test"), 0, false),
            (String::from("\u{7400}\u{6500}\u{7300}\u{7400}"), 1, true)
        ],
        extracted
    );
}

#[test]
fn test_multiple_encodings() {
    let config = BytesConfig::new(b"ascii\x00\x00\xdct\x00e\x00s\x00t\x00\x00\x00".to_vec())
        .with_encoding(Encoding::ASCII)
        .with_encoding(Encoding::UTF16LE);
    let extracted = extracted(&config);
    assert!(extracted.contains(&(String::from("ascii"), 0, false)));
    assert!(
        extracted.contains(&(String::from("test"), 8, false)),
        "{extracted:?}"
    );
    // Pairs from offset 7 are dc 74, 00 65, 00 73, 00 74.
    assert!(
        extracted.contains(&(String::from("\u{74dc}\u{6500}\u{7300}\u{7400}"), 7, true)),
        "{extracted:?}"
    );
}

#[test]
fn test_json_dump() {
    let file = NamedTempFile::new().unwrap();
    let config =
        BytesConfig::new(b"\x00\x00test\"\n\tmore\x00\x00".to_vec()).with_encoding(Encoding::ASCII);

    let path = file.path().to_path_buf();
    dump_strings(&config, path).unwrap();
    let mut string = String::new();
    file.as_file().read_to_string(&mut string).unwrap();
    assert_eq!(
        string,
        String::from("[[\"test\\\"\",2,false],[\"\\tmore\",8,false]]")
    );
}

#[test]
fn test_json_dump_multiple_strings() {
    let file = NamedTempFile::new().unwrap();
    let config = BytesConfig::new(b"\x00\x00test\"\n\tmore\x00\x00more text over here".to_vec())
        .with_encoding(Encoding::ASCII);

    let path = file.path().to_path_buf();
    dump_strings(&config, path).unwrap();
    let mut string = String::new();
    file.as_file().read_to_string(&mut string).unwrap();
    assert_eq!(
        string,
        String::from(
            "[[\"test\\\"\",2,false],[\"\\tmore\",8,false],[\"more text over here\",15,false]]"
        )
    );
}

struct FailingAfterData {
    data: Option<&'static [u8]>,
}

impl Read for FailingAfterData {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(data) = self.data.take() {
            buffer[..data.len()].copy_from_slice(data);
            Ok(data.len())
        } else {
            Err(io::Error::other("injected input failure"))
        }
    }
}

struct PartialConfig;

impl Config for PartialConfig {
    fn scan_with<S>(&self, sink: &mut S) -> Result<ScanSummary, ScanError<S::Error>>
    where
        S: StringSink,
    {
        let mut reader = FailingAfterData {
            data: Some(b"first\0second"),
        };
        scan(
            &mut reader,
            &ScanOptions::new(3, [Encoding::ASCII]).unwrap(),
            sink,
        )
    }

    fn get_min_length(&self) -> usize {
        3
    }

    fn get_encodings(&self) -> Vec<Encoding> {
        vec![Encoding::ASCII]
    }
}

#[test]
fn dump_writes_completed_hits_before_the_scan_finishes() {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().to_path_buf();
    let error = dump_strings(&PartialConfig, path).unwrap_err();
    assert!(matches!(error, ScanError::Reader(_)));

    let mut output = String::new();
    file.as_file().read_to_string(&mut output).unwrap();
    assert_eq!(output, "[[\"first\",0,false]");
}

#[test]
fn json_dump_marks_shadowed_readings() {
    let file = NamedTempFile::new().unwrap();
    // Pairs from offset 1 are 00 65, 00 73, 00 74: the swapped "est", shadowed by "test". Its
    // terminator 00 00 at offsets 7 and 8 comes first, so it finishes and is written first.
    let config =
        BytesConfig::new(b"t\x00e\x00s\x00t\x00\x00\x00".to_vec()).with_encoding(Encoding::UTF16LE);

    dump_strings(&config, file.path().to_path_buf()).unwrap();
    let mut output = String::new();
    file.as_file().read_to_string(&mut output).unwrap();
    assert_eq!(
        output,
        "[[\"\u{6500}\u{7300}\u{7400}\",1,true],[\"test\",0,false]]"
    );
}

#[test]
fn test_utf16le_with_printable_prefix() {
    // Test the bug fix: "At\x00e\x00s\x00t\x00\x00\x00" should extract "test" at offset 1
    // The 'A' at offset 0 should be skipped because 't' at offset 1 starts a valid UTF-16LE sequence
    let config = BytesConfig::new(b"At\x00e\x00s\x00t\x00\x00\x00".to_vec())
        .with_encoding(Encoding::UTF16LE);
    let extracted = extracted(&config);
    // Pairs from offset 0 are 41 74, 00 65, 00 73, 00 74, shadowed by "test".
    assert_eq!(
        vec![
            (String::from("\u{7441}\u{6500}\u{7300}\u{7400}"), 0, true),
            (String::from("test"), 1, false)
        ],
        extracted
    );
}

#[test]
fn test_utf16be_with_printable_prefix() {
    // Test the bug fix for UTF-16BE: "A\x00t\x00e\x00s\x00t\x00\x00" should extract "test" at offset 1
    // The 'A' at offset 0 should be skipped because '\x00t' at offset 1-2 starts a valid UTF-16BE sequence
    let config = BytesConfig::new(b"A\x00t\x00e\x00s\x00t\x00\x00".to_vec())
        .with_encoding(Encoding::UTF16BE);
    let extracted = extracted(&config);
    // Pairs from offset 0 are 41 00, 74 00, 65 00, 73 00, 74 00, shadowed by "test".
    assert_eq!(
        vec![
            (
                String::from("\u{4100}\u{7400}\u{6500}\u{7300}\u{7400}"),
                0,
                true
            ),
            (String::from("test"), 1, false)
        ],
        extracted
    );
}

#[test]
fn test_utf16le_with_trailing_printable() {
    // Test UTF-16LE with trailing printable without null: "t\x00e\x00s\x00t\x00A" should extract "test" at offset 0
    // The 'A' at the end doesn't have a trailing \x00, so it's not valid UTF-16LE and should stop the extraction
    let config =
        BytesConfig::new(b"t\x00e\x00s\x00t\x00A".to_vec()).with_encoding(Encoding::UTF16LE);
    let extracted = extracted(&config);
    // Pairs from offset 1 are 00 65, 00 73, 00 74, 00 41, shadowed by "test".
    assert_eq!(
        vec![
            (String::from("test"), 0, false),
            (String::from("\u{6500}\u{7300}\u{7400}\u{4100}"), 1, true)
        ],
        extracted
    );
}

#[test]
fn test_utf16be_with_trailing_printable() {
    // Test UTF-16BE with trailing printable without null: "\x00t\x00e\x00s\x00tA" should extract "test" at offset 0
    // The 'A' at the end doesn't have a \x00 before it, so it's not valid UTF-16BE and should stop the extraction
    let config =
        BytesConfig::new(b"\x00t\x00e\x00s\x00tA".to_vec()).with_encoding(Encoding::UTF16BE);
    let extracted = extracted(&config);
    // Pairs from offset 1 are 74 00, 65 00, 73 00, 74 41, shadowed by "test".
    assert_eq!(
        vec![
            (String::from("test"), 0, false),
            (String::from("\u{7400}\u{6500}\u{7300}\u{7441}"), 1, true)
        ],
        extracted
    );
}
