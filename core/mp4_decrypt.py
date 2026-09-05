"""CENC MP4/M4A 解密: spade_a 密钥派生、AES-CTR 与加密 box 清理。

短剧视频与音频共用同一套 CENC 结构, 因此本模块对两者通用。
"""

from __future__ import annotations

import base64
import binascii
import re
import struct
from dataclasses import dataclass

from Crypto.Cipher import AES

_HEX_KEY_RE = re.compile(r'^[0-9a-fA-F]{32}$')
_DROP_BOXES = frozenset({'senc', 'saio', 'saiz', 'sinf', 'schi', 'tenc', 'schm', 'frma', 'uuid'})
_CONTAINER_BOXES = frozenset({'moov', 'trak', 'mdia', 'minf', 'stbl', 'stsd', 'edts', 'mvex'})
_SAMPLE_ENTRIES = frozenset({'encv', 'enca', 'avc1', 'avc3', 'mp4a', 'hvc1', 'hev1'})


@dataclass(frozen=True, slots=True)
class Box:
  offset: int
  size: int
  data: bytes


@dataclass(frozen=True, slots=True)
class SampleEntry:
  offset: int
  size: int


@dataclass(frozen=True, slots=True)
class StscEntry:
  first_chunk: int
  samples_per_chunk: int


def _u32(data: bytes | bytearray, offset: int) -> int:
  if offset < 0 or offset + 4 > len(data):
    raise ValueError('MP4 box 数据越界')
  return struct.unpack_from('>I', data, offset)[0]


def _box_bytes(box_type: str, payload: bytes) -> bytes:
  return struct.pack('>I4s', len(payload) + 8, box_type.encode('ascii')) + payload


def _is_hex_key(value: str) -> bool:
  return bool(_HEX_KEY_RE.fullmatch(value))


def _decode_base36(value: int) -> int:
  if 48 <= value <= 57:
    return value - 48
  if 97 <= value <= 122:
    return value - 97 + 10
  return 0xFF


def _decrypt_spade_inner(spade_key: bytes) -> bytes:
  buffer = b'\xFA\x55' + spade_key
  result = bytearray(len(spade_key))
  for index, value in enumerate(spade_key):
    decoded = (value ^ buffer[index]) - (index & 0xFFFFFFFF).bit_count() - 21
    while decoded < 0:
      decoded += 0xFF
    result[index] = decoded & 0xFF
  return bytes(result)


def _decrypt_spade(data: bytes) -> str:
  if len(data) < 3:
    return ''
  padding_len = (data[0] ^ data[1] ^ data[2]) - 48
  if padding_len < 0 or len(data) < padding_len + 2:
    return ''
  inner = _decrypt_spade_inner(data[1:len(data) - padding_len])
  if not inner:
    return ''
  skip = _decode_base36(inner[0])
  end_index = 1 + len(data) - padding_len - 2 - skip
  if end_index < 1 or end_index > len(inner):
    return ''
  try:
    return inner[1:end_index].decode()
  except UnicodeDecodeError:
    return ''


def derive_key_from_spade_a(spade_a: str) -> str:
  """从 ``spade_a`` 派生 32 位小写十六进制 AES key。"""
  try:
    decoded = base64.b64decode(spade_a, validate=True)
  except (binascii.Error, ValueError):
    decoded = b''
  if decoded:
    result = _decrypt_spade(decoded)
    if _is_hex_key(result):
      return result.lower()
  if _is_hex_key(spade_a):
    return spade_a.lower()
  raise ValueError('spade_a 派生密钥失败')


def decrypt_sample_key(spade_a: str) -> bytes:
  key = bytes.fromhex(derive_key_from_spade_a(spade_a))
  if len(key) != 16:
    raise ValueError('密钥解码失败')
  return key


def aes_ctr_decrypt(data: bytes, key: bytes, iv: bytes) -> bytes:
  """按原实现的 64 位低位计数器语义执行 AES-CTR。"""
  if len(key) != 16:
    raise ValueError('AES key 必须为 16 字节')
  if len(iv) != 16:
    raise ValueError('AES IV 必须为 16 字节')
  if not data:
    return data
  cipher = AES.new(key, AES.MODE_ECB)
  high, low = struct.unpack('>QQ', iv)
  output = bytearray(len(data))
  for block_index, start in enumerate(range(0, len(data), 16)):
    counter = struct.pack('>QQ', high, (low + block_index) & 0xFFFFFFFFFFFFFFFF)
    stream = cipher.encrypt(counter)
    block = data[start:start + 16]
    output[start:start + len(block)] = bytes(a ^ b for a, b in zip(block, stream))
  return bytes(output)


def _find_box(data: bytes | bytearray, box_type: str, start: int, end: int) -> Box | None:
  position = start
  while position + 8 <= end:
    size = _u32(data, position)
    if size < 8 or size > end - position:
      break
    if bytes(data[position + 4:position + 8]).decode('latin1') == box_type:
      return Box(position, size, bytes(data[position + 8:position + size]))
    position += size
  return None


def _find_all_boxes(data: bytes | bytearray, box_type: str, start: int, end: int) -> list[Box]:
  found: list[Box] = []
  position = start
  while position + 8 <= end:
    size = _u32(data, position)
    if size < 8 or size > end - position:
      break
    if bytes(data[position + 4:position + 8]).decode('latin1') == box_type:
      found.append(Box(position, size, bytes(data[position + 8:position + size])))
    position += size
  return found


def _parse_stsz(data: bytes) -> list[int]:
  sample_size = _u32(data, 4)
  count = _u32(data, 8)
  if sample_size:
    return [sample_size] * count
  if 12 + count * 4 > len(data):
    raise ValueError('stsz 数据不完整')
  return [_u32(data, 12 + index * 4) for index in range(count)]


def _parse_stsc(data: bytes) -> list[StscEntry]:
  count = _u32(data, 4)
  if 8 + count * 12 > len(data):
    raise ValueError('stsc 数据不完整')
  return [StscEntry(_u32(data, 8 + index * 12), _u32(data, 12 + index * 12)) for index in range(count)]


def _parse_stco(data: bytes) -> list[int]:
  count = _u32(data, 4)
  if 8 + count * 4 > len(data):
    raise ValueError('stco 数据不完整')
  return [_u32(data, 8 + index * 4) for index in range(count)]


def _parse_co64(data: bytes) -> list[int]:
  count = _u32(data, 4)
  if 8 + count * 8 > len(data):
    raise ValueError('co64 数据不完整')
  return [struct.unpack_from('>Q', data, 8 + index * 8)[0] for index in range(count)]


def _parse_senc(data: bytes) -> list[bytes]:
  count = _u32(data, 4)
  if 8 + count * 8 > len(data):
    raise ValueError('senc 数据不完整')
  return [data[8 + index * 8:16 + index * 8] + bytes(8) for index in range(count)]


def _build_sample_map(sizes: list[int], stsc: list[StscEntry], chunk_offsets: list[int]) -> list[SampleEntry]:
  sample_map: list[SampleEntry] = []
  sample_index = 0
  for chunk_index, chunk_offset in enumerate(chunk_offsets, start=1):
    samples_per_chunk = 0
    for entry_index, entry in enumerate(stsc):
      next_first = stsc[entry_index + 1].first_chunk if entry_index + 1 < len(stsc) else None
      if chunk_index >= entry.first_chunk and (next_first is None or chunk_index < next_first):
        samples_per_chunk = entry.samples_per_chunk
        break
    offset = chunk_offset
    for _ in range(samples_per_chunk):
      if sample_index >= len(sizes):
        break
      size = sizes[sample_index]
      sample_map.append(SampleEntry(offset, size))
      offset += size
      sample_index += 1
  return sample_map


def _is_printable_box_type(value: bytes) -> bool:
  return len(value) == 4 and all(0x20 <= byte <= 0x7E for byte in value)


def _find_next_box(data: bytes | bytearray, start: int, end: int) -> int:
  for position in range(start, max(start, end - 7)):
    size = _u32(data, position)
    box_type = bytes(data[position + 4:position + 8])
    if size >= 8 and size <= end - position and _is_printable_box_type(box_type):
      return position
  return -1


def _find_frma_codec(data: bytes | bytearray, offset: int, size: int) -> str:
  end = offset + size
  position = offset + 16
  while position < end:
    next_box = _find_next_box(data, position, end)
    if next_box < 0:
      break
    box_size = _u32(data, next_box)
    if bytes(data[next_box + 4:next_box + 8]) == b'sinf' and box_size >= 16:
      inner_position = next_box + 8
      inner_end = next_box + box_size
      while inner_position < inner_end:
        child = _find_next_box(data, inner_position, inner_end)
        if child < 0:
          break
        child_size = _u32(data, child)
        if bytes(data[child + 4:child + 8]) == b'frma' and child_size >= 12:
          return bytes(data[child + 8:child + 12]).decode('latin1')
        inner_position = child + child_size
    position = next_box + box_size
  return ''


def _process_box_entries(data: bytes | bytearray, start: int, end: int) -> bytes:
  parts = bytearray()
  position = start
  while position < end:
    if position + 8 > end:
      parts.extend(data[position:end])
      break
    size = _u32(data, position)
    box_type_bytes = bytes(data[position + 4:position + 8])
    if size < 8 or size > end - position or not _is_printable_box_type(box_type_bytes):
      next_box = _find_next_box(data, position + 1, end)
      if next_box < 0:
        parts.extend(data[position:end])
        break
      parts.extend(data[position:next_box])
      position = next_box
      continue
    box_type = box_type_bytes.decode('ascii')
    if box_type in _DROP_BOXES:
      position += size
      continue
    if box_type in {'encv', 'enca'}:
      codec = _find_frma_codec(data, position, size) or ('avc1' if box_type == 'encv' else 'mp4a')
      parts.extend(_box_bytes(codec, _process_box_tree(data, position, size)))
    elif box_type in _CONTAINER_BOXES:
      parts.extend(_box_bytes(box_type, _process_box_tree(data, position, size)))
    else:
      parts.extend(data[position:position + size])
    position += size
  return bytes(parts)


def _process_box_tree(data: bytes | bytearray, offset: int, size: int) -> bytes:
  box_type = bytes(data[offset + 4:offset + 8]).decode('latin1')
  if box_type == 'stsd':
    return bytes(data[offset + 8:offset + 16]) + _process_box_entries(data, offset + 16, offset + size)
  if box_type in _SAMPLE_ENTRIES:
    return bytes(data[offset + 8:offset + 16]) + _process_box_entries(data, offset + 16, offset + size)
  return _process_box_entries(data, offset + 8, offset + size)


def _replace_codec_in_place(data: bytearray, offset: int, size: int) -> None:
  box_type = bytes(data[offset + 4:offset + 8]).decode('latin1')
  if box_type in {'encv', 'enca'}:
    codec = _find_frma_codec(data, offset, size) or ('avc1' if box_type == 'encv' else 'mp4a')
    data[offset + 4:offset + 8] = codec.encode('latin1')
  if box_type == 'stsd':
    entry_count = _u32(data, offset + 12)
    position = offset + 16
    for _ in range(entry_count):
      if position + 8 > offset + size:
        break
      entry_size = _u32(data, position)
      if entry_size < 8 or entry_size > offset + size - position:
        break
      _replace_codec_in_place(data, position, entry_size)
      position += entry_size
    return
  position = offset + 8
  end = offset + size
  while position + 8 <= end:
    child_size = _u32(data, position)
    if child_size < 8 or child_size > end - position:
      break
    _replace_codec_in_place(data, position, child_size)
    position += child_size


def decrypt_mp4(file_data: bytes, key_hex: str) -> bytes:
  """解密非 fragmented CENC MP4,并清理加密元数据。"""
  if not _is_hex_key(key_hex):
    raise ValueError('无效 Hex Key')
  mutable = bytearray(file_data)
  key = bytes.fromhex(key_hex)
  moov = _find_box(mutable, 'moov', 0, len(mutable))
  if moov is None:
    raise ValueError('未找到 moov')
  tracks = _find_all_boxes(mutable, 'trak', moov.offset + 8, moov.offset + moov.size)
  if not tracks:
    raise ValueError('未找到 trak')

  total_decrypted = 0
  for track in tracks:
    track_start, track_end = track.offset + 8, track.offset + track.size
    mdia = _find_box(mutable, 'mdia', track_start, track_end)
    if mdia is None:
      continue
    minf = _find_box(mutable, 'minf', mdia.offset + 8, mdia.offset + mdia.size)
    if minf is None:
      continue
    stbl = _find_box(mutable, 'stbl', minf.offset + 8, minf.offset + minf.size)
    if stbl is None:
      continue
    start, end = stbl.offset + 8, stbl.offset + stbl.size
    stsz = _find_box(mutable, 'stsz', start, end)
    stsc = _find_box(mutable, 'stsc', start, end)
    stco = _find_box(mutable, 'stco', start, end)
    co64 = None if stco else _find_box(mutable, 'co64', start, end)
    if stsz is None or stsc is None or (stco is None and co64 is None):
      continue
    senc = _find_box(mutable, 'senc', start, end) or _find_box(mutable, 'senc', track_start, track_end)
    if senc is None:
      continue
    sizes = _parse_stsz(stsz.data)
    stsc_entries = _parse_stsc(stsc.data)
    chunk_offsets = _parse_stco(stco.data) if stco else _parse_co64(co64.data)
    ivs = _parse_senc(senc.data)
    for index, sample in enumerate(_build_sample_map(sizes, stsc_entries, chunk_offsets)):
      if sample.size <= 0 or sample.offset < 0 or sample.offset + sample.size > len(mutable):
        continue
      iv = ivs[index] if index < len(ivs) else bytes(16)
      mutable[sample.offset:sample.offset + sample.size] = aes_ctr_decrypt(
        bytes(mutable[sample.offset:sample.offset + sample.size]), key, iv,
      )
      total_decrypted += 1

  if total_decrypted == 0:
    raise ValueError('没有任何 sample 被解密(结构可能异常)')

  clean_payload = _process_box_tree(mutable, moov.offset, moov.size)
  clean_moov = _box_bytes('moov', clean_payload)
  if len(clean_moov) < moov.size:
    padding = moov.size - len(clean_moov)
    if padding >= 8:
      clean_moov += _box_bytes('free', bytes(padding - 8))
    else:
      _replace_codec_in_place(mutable, moov.offset, moov.size)
      return bytes(mutable)
  elif len(clean_moov) > moov.size:
    _replace_codec_in_place(mutable, moov.offset, moov.size)
    return bytes(mutable)

  mutable[moov.offset:moov.offset + moov.size] = clean_moov
  return bytes(mutable)
