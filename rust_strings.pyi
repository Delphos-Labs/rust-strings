from pathlib import Path
from typing import List, Optional, Union


class StringHit:
    @property
    def text(self) -> str: ...
    @property
    def source_offset(self) -> int: ...
    @property
    def source_byte_length(self) -> int: ...
    @property
    def encoding(self) -> str: ...
    @property
    def character_count(self) -> int: ...
    @property
    def decoded_utf8_length(self) -> int: ...


def strings(
    file_path: Optional[Union[str, Path]] = None,
    bytes: Optional[bytes] = None,
    min_length: int = 3,
    encodings: List[str] = ["ascii"],
    buffer_size: int = 1024 * 1024,
) -> List[StringHit]:
    """
    Extract strings from binary file or bytes.
    :param file_path: path to file (can't be with bytes option)
    :param bytes: bytes (can't be with file_path option)
    :param min_length: strings minimum length
    :param encodings: strings encodings (default is ["ascii"])
    :param buffer_size: the buffer size to read the file (relevant only to file_path option)
    :return: typed hits with source and decoded metadata
    :raises: raise StringsException if there is any error during string extraction
             raise EncodingNotFoundException if the function got an unsupported encondings
    """
    ...


def dump_strings(
    output_file: Union[str, Path],
    file_path: Optional[Union[str, Path]] = None,
    bytes: Optional[bytes] = None,
    min_length: int = 3,
    encodings: List[str] = ["ascii"],
    buffer_size: int = 1024 * 1024,
) -> None:
    """
    Dump strings from binary file or bytes to json file.
    :param output_file: path to file to dump into
    :param file_path: path to file (can't be with bytes option)
    :param bytes: bytes (can't be with file_path option)
    :param min_length: strings minimum length
    :param encodings: strings encodings (default is ["ascii"])
    :param buffer_size: the buffer size to read the file (relevant only to file_path option)
    :return: None
    :raises: raise StringsException if there is any error during string extraction
             raise EncodingNotFoundException if the function got an unsupported encondings
    """
    ...
