import json
import os
from pathlib import Path
from uuid import uuid4

import pytest

import rust_strings


@pytest.fixture
def temp_file(tmp_path: Path) -> Path:
    file = tmp_path / str(uuid4())
    yield file
    os.remove(file)


def pairs(hits):
    return [(hit.text, hit.source_offset) for hit in hits]


def test_bytes():
    extracted = rust_strings.strings(bytes=b"test\x00")
    assert isinstance(extracted[0], rust_strings.StringHit)
    assert pairs(extracted) == [("test", 0)]
    assert extracted[0].source_byte_length == 4
    assert extracted[0].encoding == "ascii"
    assert extracted[0].character_count == 4
    assert extracted[0].decoded_utf8_length == 4


def test_bytes_min_length_1():
    extracted = rust_strings.strings(bytes=b"test\x00", min_length=1)
    assert pairs(extracted) == [("test", 0)]


def test_single_byte():
    extracted = rust_strings.strings(bytes=b"t\x00", min_length=1)
    assert pairs(extracted) == [("t", 0)]


def test_bytes_with_offset():
    extracted = rust_strings.strings(bytes=b"\x00test")
    assert pairs(extracted) == [("test", 1)]


def test_bytes_multiple():
    extracted = rust_strings.strings(bytes=b"\x00test\x00test")
    assert pairs(extracted) == [("test", 1), ("test", 6)]


def test_file(temp_file: Path):
    temp_file.write_bytes(b"test\x00")
    extracted = rust_strings.strings(file_path=temp_file)
    assert pairs(extracted) == [("test", 0)]


def test_file_as_str(temp_file: Path):
    temp_file.write_bytes(b"test\x00")
    extracted = rust_strings.strings(file_path=str(temp_file))
    assert pairs(extracted) == [("test", 0)]


def test_multiple_encodings():
    extracted = rust_strings.strings(
        bytes=b"ascii\x00\x00\xdct\x00e\x00s\x00t\x00\x00\x00",
        encodings=["ascii", "utf-16le"],
    )
    assert ("ascii", 0) in pairs(extracted)
    assert ("test", 8) in pairs(extracted)
    utf16 = next(hit for hit in extracted if hit.text == "test")
    assert utf16.encoding == "utf-16le"
    assert utf16.source_byte_length == 8


def test_json_dump(temp_file: Path):
    rust_strings.dump_strings(temp_file, bytes=b'\x00\x00test"\n\tmore\x00\x00')
    assert json.loads(temp_file.read_text()) == [['test"\n\tmore', 2]]


def test_json_dump_multiple_strings(temp_file: Path):
    rust_strings.dump_strings(
        temp_file, bytes=b'\x00\x00test"\n\tmore\x00\x00more text over here'
    )
    assert json.loads(temp_file.read_text()) == [
        ['test"\n\tmore', 2],
        ["more text over here", 15],
    ]
