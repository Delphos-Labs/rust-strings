use std::collections::HashMap;
use std::convert::Infallible;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Write};
use std::path::{Path, PathBuf};

use crate::{scan, Encoding, HitFinish, HitId, HitStart, ScanOptions, SinkControl, StringSink};

const DEFAULT_MIN_LENGTH: usize = 3;
const DEFAULT_ENCODINGS: [Encoding; 2] = [Encoding::ASCII, Encoding::UTF16LE];

pub trait Config {
    #[doc(hidden)]
    fn scan_with<S>(&self, sink: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: StringSink,
        S::Error: Error + 'static;

    #[doc(hidden)]
    fn get_min_length(&self) -> usize;

    #[doc(hidden)]
    fn get_encodings(&self) -> Vec<Encoding>;
}

macro_rules! impl_config_accessors {
    () => {
        fn get_min_length(&self) -> usize {
            self.min_length
        }

        fn get_encodings(&self) -> Vec<Encoding> {
            if self.encodings.is_empty() {
                DEFAULT_ENCODINGS.to_vec()
            } else {
                self.encodings.clone()
            }
        }
    };
}

macro_rules! impl_builders {
    () => {
        pub fn with_min_length(mut self, min_length: usize) -> Self {
            self.min_length = min_length;
            self
        }

        pub fn with_encoding(mut self, encoding: Encoding) -> Self {
            self.encodings.push(encoding);
            self
        }

        pub fn with_encodings(mut self, encodings: Vec<Encoding>) -> Self {
            self.encodings = encodings;
            self
        }
    };
}

pub struct FileConfig<'a> {
    pub file_path: &'a Path,
    pub min_length: usize,
    pub encodings: Vec<Encoding>,
    pub buffer_size: usize,
}

impl<'a> FileConfig<'a> {
    const DEFAULT_BUFFER_SIZE: usize = 1024 * 1024;

    pub fn new(file_path: &'a Path) -> Self {
        Self {
            file_path,
            min_length: DEFAULT_MIN_LENGTH,
            encodings: Vec::new(),
            buffer_size: Self::DEFAULT_BUFFER_SIZE,
        }
    }

    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    impl_builders!();
}

impl Config for FileConfig<'_> {
    fn scan_with<S>(&self, sink: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: StringSink,
        S::Error: Error + 'static,
    {
        let options = ScanOptions::new(self.get_min_length(), self.get_encodings())?;
        let file = File::open(self.file_path)?;
        let mut reader = BufReader::with_capacity(self.buffer_size, file);
        scan(&mut reader, &options, sink)?;
        Ok(())
    }

    impl_config_accessors!();
}

pub struct StdinConfig {
    pub min_length: usize,
    pub encodings: Vec<Encoding>,
    pub buffer_size: usize,
}

impl Default for StdinConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinConfig {
    const DEFAULT_BUFFER_SIZE: usize = 1024 * 1024;

    pub fn new() -> Self {
        Self {
            min_length: DEFAULT_MIN_LENGTH,
            encodings: Vec::new(),
            buffer_size: Self::DEFAULT_BUFFER_SIZE,
        }
    }

    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    impl_builders!();
}

impl Config for StdinConfig {
    fn scan_with<S>(&self, sink: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: StringSink,
        S::Error: Error + 'static,
    {
        let options = ScanOptions::new(self.get_min_length(), self.get_encodings())?;
        let stdin = io::stdin();
        let mut reader = BufReader::with_capacity(self.buffer_size, stdin.lock());
        scan(&mut reader, &options, sink)?;
        Ok(())
    }

    impl_config_accessors!();
}

pub struct BytesConfig {
    pub bytes: Vec<u8>,
    pub min_length: usize,
    pub encodings: Vec<Encoding>,
}

impl BytesConfig {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            min_length: DEFAULT_MIN_LENGTH,
            encodings: Vec::new(),
        }
    }

    impl_builders!();
}

impl Config for BytesConfig {
    fn scan_with<S>(&self, sink: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: StringSink,
        S::Error: Error + 'static,
    {
        let options = ScanOptions::new(self.get_min_length(), self.get_encodings())?;
        let mut reader = Cursor::new(self.bytes.as_slice());
        scan(&mut reader, &options, sink)?;
        Ok(())
    }

    impl_config_accessors!();
}

#[derive(Default)]
struct VectorSink {
    active: HashMap<HitId, (String, u64)>,
    complete: Vec<(HitId, String, u64)>,
}

impl StringSink for VectorSink {
    type Error = Infallible;

    fn start(&mut self, id: HitId, hit: HitStart) -> Result<SinkControl, Self::Error> {
        self.active.insert(id, (String::new(), hit.offset.get()));
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, id: HitId, text: &str) -> Result<SinkControl, Self::Error> {
        self.active
            .get_mut(&id)
            .expect("active hit")
            .0
            .push_str(text);
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, id: HitId, _hit: HitFinish) -> Result<(), Self::Error> {
        let (text, offset) = self.active.remove(&id).expect("active hit");
        self.complete.push((id, text, offset));
        Ok(())
    }

    fn abort(&mut self, id: HitId) -> Result<(), Self::Error> {
        self.active.remove(&id);
        Ok(())
    }
}

pub fn strings<T: Config>(config: &T) -> Result<Vec<(String, u64)>, Box<dyn Error>> {
    let mut sink = VectorSink::default();
    config.scan_with(&mut sink)?;
    sink.complete
        .sort_by_key(|(id, _, offset)| (*offset, id.get()));
    Ok(sink
        .complete
        .into_iter()
        .map(|(_, text, offset)| (text, offset))
        .collect())
}

pub fn dump_strings<T: Config>(config: &T, output: PathBuf) -> Result<(), Box<dyn Error>> {
    let strings = strings(config)?;
    let mut writer = File::create(output)?;
    writer.write_all(b"[")?;
    for (index, (text, offset)) in strings.iter().enumerate() {
        if index != 0 {
            writer.write_all(b",")?;
        }
        writer.write_all(b"[\"")?;
        write_json_string_contents(&mut writer, text)?;
        write!(writer, "\",{offset}]")?;
    }
    writer.write_all(b"]")?;
    Ok(())
}

fn write_json_string_contents(writer: &mut impl Write, text: &str) -> io::Result<()> {
    for character in text.chars() {
        match character {
            '"' => writer.write_all(b"\\\"")?,
            '\\' => writer.write_all(b"\\\\")?,
            '\n' => writer.write_all(b"\\n")?,
            '\r' => writer.write_all(b"\\r")?,
            '\t' => writer.write_all(b"\\t")?,
            character if character <= '\u{1f}' => {
                write!(writer, "\\u{:04x}", u32::from(character))?
            }
            character => {
                let mut encoded = [0_u8; 4];
                writer.write_all(character.encode_utf8(&mut encoded).as_bytes())?;
            }
        }
    }
    Ok(())
}
