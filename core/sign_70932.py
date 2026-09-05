# 70932 native registerkey signing engine
# Built from url_params_sign_client code, adapted only to work with registerkey endpoint.
# Change: sign_key uses key accepted by reading/crypt/registerkey.
# All other logic (SM3, Simon, AES, protobuf, X-Argus bean) is standard 70932.

import base64
import hashlib
import os
import struct
import time
from copy import deepcopy
from random import randint
from typing import Dict, List, Optional
from urllib.parse import parse_qs

# =============== sign_key accepted by registerkey ===============
_SIGN_KEY = bytes.fromhex("ac1adaae95a7af94a5114ab3b3a97dd80050aa0a39314c40528caec95256c28c")
_SM3_OUTPUT = bytes.fromhex("fc78e0a9657a0c748ce51559903ccf03510e51d3cff232d71343e88a321c5304")
_SIMON_SEED = 0x3DC94C3A046D678B
_MAGIC = bytes([0xA6, 0x6E, 0xAD, 0x9F, 0x77, 0x01, 0xD0, 0x0C, 0x18])
_MARKER = bytes([0xF2, 0xF7, 0xFC, 0xFF, 0xF2, 0xF7, 0xFC, 0xFF])
_XG_HEX_STR = [30, 64, 224, 217, 147, 69, 0, 180]
_XG_LEN = 20

# =============== SM3 ===============
class SM3:
    def __init__(self):
        self.IV = [1937774191, 1226093241, 388252375, 3666478592, 2842636476, 372324522, 3817729613, 2969243214]
        self.TJ = [2043430169] * 16 + [2055708042] * 48

    @staticmethod
    def rotate_left(a: int, k: int) -> int:
        k %= 32
        return ((a << k) & 0xFFFFFFFF) | ((a & 0xFFFFFFFF) >> (32 - k))

    @staticmethod
    def ffj(x: int, y: int, z: int, j: int) -> int:
        return x ^ y ^ z if 0 <= j < 16 else (x & y) | (x & z) | (y & z)

    @staticmethod
    def ggj(x: int, y: int, z: int, j: int) -> int:
        return x ^ y ^ z if 0 <= j < 16 else (x & y) | ((~x) & z)

    def p0(self, x: int) -> int:
        return x ^ self.rotate_left(x, 9) ^ self.rotate_left(x, 17)

    def p1(self, x: int) -> int:
        return x ^ self.rotate_left(x, 15) ^ self.rotate_left(x, 23)

    def cf(self, v_i: list, b_i: bytearray) -> list:
        w = []
        for i in range(16):
            weight = 0x1000000
            data = 0
            for k in range(i * 4, (i + 1) * 4):
                data += b_i[k] * weight
                weight = int(weight / 0x100)
            w.append(data)
        for j in range(16, 68):
            w.append(
                self.p1(w[j - 16] ^ w[j - 9] ^ self.rotate_left(w[j - 3], 15))
                ^ self.rotate_left(w[j - 13], 7)
                ^ w[j - 6]
            )
        w1 = [w[j] ^ w[j + 4] for j in range(64)]
        a, b, c, d, e, f, g, h = v_i
        for j in range(64):
            ss1 = self.rotate_left((self.rotate_left(a, 12) + e + self.rotate_left(self.TJ[j], j)) & 0xFFFFFFFF, 7)
            ss2 = ss1 ^ self.rotate_left(a, 12)
            tt1 = (self.ffj(a, b, c, j) + d + ss2 + w1[j]) & 0xFFFFFFFF
            tt2 = (self.ggj(e, f, g, j) + h + ss1 + w[j]) & 0xFFFFFFFF
            d, c, b, a = c, self.rotate_left(b, 9), a, tt1
            h, g, f, e = g, self.rotate_left(f, 19), e, self.p0(tt2)
        return [
            a ^ v_i[0], b ^ v_i[1], c ^ v_i[2], d ^ v_i[3],
            e ^ v_i[4], f ^ v_i[5], g ^ v_i[6], h ^ v_i[7],
        ]

    def sm3_hash(self, msg: bytes) -> bytes:
        msg = bytearray(msg)
        length = len(msg)
        reserve = length % 64
        msg.append(0x80)
        reserve += 1
        range_end = 56
        if reserve > range_end:
            range_end += 64
        for _ in range(reserve, range_end):
            msg.append(0)
        bit_length = length * 8
        bit_length_str = [bit_length % 0x100]
        for _ in range(7):
            bit_length = int(bit_length / 0x100)
            bit_length_str.append(bit_length % 0x100)
        for i in range(8):
            msg.append(bit_length_str[7 - i])
        blocks = [msg[i * 64:(i + 1) * 64] for i in range(round(len(msg) / 64))]
        values = [self.IV]
        for block in blocks:
            values.append(self.cf(values[-1], block))
        return b"".join(int(item).to_bytes(4, "big") for item in values[-1])


# =============== Simon ===============
def _rotr64(v: int, n: int) -> int:
    return ((v >> (n % 64)) | (v << (64 - (n % 64)))) & 0xFFFFFFFFFFFFFFFF


def _rotl64(v: int, n: int) -> int:
    return ((v << (n % 64)) | (v >> (64 - (n % 64)))) & 0xFFFFFFFFFFFFFFFF


def _simon_key_expansion(key: list) -> list:
    for i in range(4, 72):
        tmp = _rotr64(key[i - 1], 3) ^ key[i - 3]
        tmp ^= _rotr64(tmp, 1)
        key[i] = (~key[i - 4] & 0xFFFFFFFFFFFFFFFF) ^ tmp ^ ((_SIMON_SEED >> ((i - 4) % 62)) & 1) ^ 3
    return key


def _simon_enc(pt: list, k: list) -> list:
    key = [0] * 72
    key[:4] = k[:4]
    key = _simon_key_expansion(key)
    x_i, x_i1 = pt
    for i in range(72):
        tmp = x_i1
        f = _rotl64(x_i1, 1) & _rotl64(x_i1, 8)
        x_i1 = x_i ^ f ^ _rotl64(x_i1, 2) ^ key[i]
        x_i = tmp
    return [x_i, x_i1]


# =============== AES-128 ===============
AES_SBOX = [
    0x63,0x7C,0x77,0x7B,0xF2,0x6B,0x6F,0xC5,0x30,0x01,0x67,0x2B,0xFE,0xD7,0xAB,0x76,
    0xCA,0x82,0xC9,0x7D,0xFA,0x59,0x47,0xF0,0xAD,0xD4,0xA2,0xAF,0x9C,0xA4,0x72,0xC0,
    0xB7,0xFD,0x93,0x26,0x36,0x3F,0xF7,0xCC,0x34,0xA5,0xE5,0xF1,0x71,0xD8,0x31,0x15,
    0x04,0xC7,0x23,0xC3,0x18,0x96,0x05,0x9A,0x07,0x12,0x80,0xE2,0xEB,0x27,0xB2,0x75,
    0x09,0x83,0x2C,0x1A,0x1B,0x6E,0x5A,0xA0,0x52,0x3B,0xD6,0xB3,0x29,0xE3,0x2F,0x84,
    0x53,0xD1,0x00,0xED,0x20,0xFC,0xB1,0x5B,0x6A,0xCB,0xBE,0x39,0x4A,0x4C,0x58,0xCF,
    0xD0,0xEF,0xAA,0xFB,0x43,0x4D,0x33,0x85,0x45,0xF9,0x02,0x7F,0x50,0x3C,0x9F,0xA8,
    0x51,0xA3,0x40,0x8F,0x92,0x9D,0x38,0xF5,0xBC,0xB6,0xDA,0x21,0x10,0xFF,0xF3,0xD2,
    0xCD,0x0C,0x13,0xEC,0x5F,0x97,0x44,0x17,0xC4,0xA7,0x7E,0x3D,0x64,0x5D,0x19,0x73,
    0x60,0x81,0x4F,0xDC,0x22,0x2A,0x90,0x88,0x46,0xEE,0xB8,0x14,0xDE,0x5E,0x0B,0xDB,
    0xE0,0x32,0x3A,0x0A,0x49,0x06,0x24,0x5C,0xC2,0xD3,0xAC,0x62,0x91,0x95,0xE4,0x79,
    0xE7,0xC8,0x37,0x6D,0x8D,0xD5,0x4E,0xA9,0x6C,0x56,0xF4,0xEA,0x65,0x7A,0xAE,0x08,
    0xBA,0x78,0x25,0x2E,0x1C,0xA6,0xB4,0xC6,0xE8,0xDD,0x74,0x1F,0x4B,0xBD,0x8B,0x8A,
    0x70,0x3E,0xB5,0x66,0x48,0x03,0xF6,0x0E,0x61,0x35,0x57,0xB9,0x86,0xC1,0x1D,0x9E,
    0xE1,0xF8,0x98,0x11,0x69,0xD9,0x8E,0x94,0x9B,0x1E,0x87,0xE9,0xCE,0x55,0x28,0xDF,
    0x8C,0xA1,0x89,0x0D,0xBF,0xE6,0x42,0x68,0x41,0x99,0x2D,0x0F,0xB0,0x54,0xBB,0x16,
]


def _aes_xtime(value: int) -> int:
    return ((value << 1) ^ 0x1B) & 0xFF if value & 0x80 else (value << 1) & 0xFF


def _aes_key_expansion_128(key: bytes) -> list:
    if len(key) != 16:
        raise ValueError("AES-128 key must be 16 bytes")
    rcon = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36]
    words = [bytearray(key[i:i+4]) for i in range(0, 16, 4)]
    for i in range(4, 44):
        temp = bytearray(words[i-1])
        if i % 4 == 0:
            temp = bytearray([AES_SBOX[temp[1]], AES_SBOX[temp[2]], AES_SBOX[temp[3]], AES_SBOX[temp[0]]])
            temp[0] ^= rcon[i // 4]
        words.append(bytearray(words[i-4][j] ^ temp[j] for j in range(4)))
    return [b''.join(bytes(b) for b in words[i:i+4]) for i in range(0, 44, 4)]


def _aes_cbc_encrypt(data: bytes, key: bytes, iv: bytes) -> bytes:
    if len(iv) != 16:
        raise ValueError("IV must be 16 bytes")
    round_keys = _aes_key_expansion_128(key)
    previous = iv
    output = bytearray()
    for offset in range(0, len(data), 16):
        block = bytes(a ^ b for a, b in zip(data[offset:offset+16], previous))
        state = bytearray(block)
        for ri, rk in enumerate(round_keys):
            for i in range(16):
                state[i] ^= rk[i]
            if ri < 10:
                for i in range(16):
                    state[i] = AES_SBOX[state[i]]
                for row in range(1, 4):
                    vals = [state[row + 4 * col] for col in range(4)]
                    vals = vals[row:] + vals[:row]
                    for col in range(4):
                        state[row + 4 * col] = vals[col]
                if ri < 9:
                    for col in range(4):
                        i = 4 * col
                        a0, a1, a2, a3 = state[i], state[i+1], state[i+2], state[i+3]
                        state[i] = _aes_xtime(a0) ^ _aes_xtime(a1) ^ a1 ^ a2 ^ a3
                        state[i+1] = a0 ^ _aes_xtime(a1) ^ _aes_xtime(a2) ^ a2 ^ a3
                        state[i+2] = a0 ^ a1 ^ _aes_xtime(a2) ^ _aes_xtime(a3) ^ a3
                        state[i+3] = _aes_xtime(a0) ^ a0 ^ a1 ^ a2 ^ _aes_xtime(a3)
        output.extend(state)
        previous = state
    return bytes(output)


# =============== PKCS7 ===============
def _pkcs7_pad(data: bytes, block_size: int = 16) -> bytes:
    pad_len = block_size - (len(data) % block_size)
    return data + bytes([pad_len]) * pad_len


# =============== ProtoBuf (70932 variant) ===============
def _write_varint(value: int) -> bytes:
    value &= 0xFFFFFFFF
    out = bytearray()
    while value > 0x80:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value & 0x7F)
    return bytes(out)


def _protobuf_encode(data: Dict[int, any]) -> bytes:
    out = bytearray()
    for idx, value in data.items():
        if isinstance(value, int):
            out += _write_varint((idx << 3) | 0)
            out += _write_varint(value)
        elif isinstance(value, str):
            raw = value.encode("utf-8")
            out += _write_varint((idx << 3) | 2)
            out += _write_varint(len(raw))
            out += raw
        elif isinstance(value, bytes):
            out += _write_varint((idx << 3) | 2)
            out += _write_varint(len(value))
            out += value
        elif isinstance(value, dict):
            raw = _protobuf_encode(value)
            out += _write_varint((idx << 3) | 2)
            out += _write_varint(len(raw))
            out += raw
    return bytes(out)


# =============== X-Argus (70932 native) ===============
def _encrypt_enc_pb(data: bytes, length: int) -> bytes:
    values = list(data)
    xor_array = values[:8]
    for i in range(8, length):
        values[i] ^= xor_array[i % 8]
    return bytes(values[::-1])


def _calculate_constant(code: str) -> int:
    digits = [int(ch) for ch in (code or "").replace(".", "").zfill(6)]
    weights = [20480, 2048, 20971520, 2097152, 1342177280, 134217728]
    return sum(p * w for p, w in zip(digits, weights))


def _argus_bodyhash(stub: Optional[str]) -> bytes:
    if stub is None or len(stub) == 0:
        return SM3().sm3_hash(bytes(16))[:6]
    return SM3().sm3_hash(bytes.fromhex(stub))[:6]


def _argus_queryhash(query: str) -> bytes:
    if not query or len(query) == 0:
        return SM3().sm3_hash(bytes(16))[:6]
    return SM3().sm3_hash(query.encode())[:6]


def _argus_encrypt(bean: Dict[int, any]) -> str:
    protobuf = _pkcs7_pad(_protobuf_encode(bean), 16)
    new_len = len(protobuf)
    key_list = []
    for i in range(2):
        key_list.extend(list(struct.unpack("<QQ", _SM3_OUTPUT[i * 16:i * 16 + 16])))
    enc_pb = bytearray(new_len)
    for i in range(new_len // 16):
        pt = list(struct.unpack("<QQ", protobuf[i * 16:i * 16 + 16]))
        ct = _simon_enc(pt, key_list)
        enc_pb[i * 16:i * 16 + 8] = ct[0].to_bytes(8, "little")
        enc_pb[i * 16 + 8:i * 16 + 16] = ct[1].to_bytes(8, "little")

    b_buffer = _encrypt_enc_pb(_MARKER + enc_pb, new_len + 8)
    b_buffer = _MAGIC + b_buffer + b"ao"
    cipher_text = _aes_cbc_encrypt(
        _pkcs7_pad(b_buffer, 16),
        hashlib.md5(_SIGN_KEY[:16]).digest(),
        hashlib.md5(_SIGN_KEY[16:]).digest(),
    )
    return base64.b64encode(b"\xf2\x81" + cipher_text).decode("utf-8")


def argus_get_sign(
    params: str,
    stub: Optional[str] = None,
    timestamp: Optional[int] = None,
    aid: int = 1967,
    license_id: int = 1611921764,
) -> str:
    query = params or ""
    ts = int(time.time()) if timestamp is None else int(timestamp)
    params_dict = parse_qs(query)
    os_version = params_dict.get("os_version", [""])[0]

    bean: Dict[int, any] = {
        1: 0x20200929 << 1,
        2: 2,
        3: randint(0, 0x7FFFFFFF),
        4: str(aid),
        5: params_dict.get("device_id", [""])[0] or "",
        6: str(license_id),
        7: params_dict.get("version_name", [""])[0] or "",
        8: "v04.04.05-ov-android",
        9: 134744640,
        10: bytes(8),
        11: 0,
        12: ts << 1,
        13: _argus_bodyhash(stub),
        14: _argus_queryhash(query),
        20: 738,
        23: {
            1: str(params_dict.get("device_type", [""])[0]),
            2: os_version,
            3: "googleplay",
            4: _calculate_constant(os_version),
        },
    }
    return _argus_encrypt(bean)


# =============== X-Ladon (70932 native) ===============
def _ladon_validate(num: int) -> int:
    return num & 0xFFFFFFFFFFFFFFFF


def _ladon_pad_size(size: int) -> int:
    mod = size % 16
    return size + (16 - mod) if mod else size


def _ladon_pad_buffer(buffer: bytearray, data_length: int, buffer_size: int, modulus: int) -> int:
    pad_byte = modulus - (data_length % modulus)
    if data_length + pad_byte > buffer_size:
        return -pad_byte
    for i in range(pad_byte):
        buffer[data_length + i] = pad_byte
    return pad_byte


def _ladon_enc_input(hash_table: bytearray, input_data: bytes) -> bytes:
    data0 = int.from_bytes(input_data[:8], "little")
    data1 = int.from_bytes(input_data[8:], "little")
    for i in range(0x22):
        h = int.from_bytes(hash_table[i * 8:(i + 1) * 8], "little")
        data1 = _ladon_validate(h ^ (data0 + _rotr64(data1, 8)))
        data0 = _ladon_validate(data1 ^ _rotr64(data0, 0x3D))
    out = bytearray(16)
    out[:8] = data0.to_bytes(8, "little")
    out[8:] = data1.to_bytes(8, "little")
    return bytes(out)


def _ladon_encrypt_data(md5hex: bytes, data: bytes, size: int) -> bytes:
    ht = bytearray(288)
    ht[:32] = md5hex
    temp = [int.from_bytes(ht[i * 8:(i + 1) * 8], "little") for i in range(4)]
    b0 = temp.pop(0)
    b8 = temp.pop(0)
    for r in range(0x22):
        x8 = _ladon_validate(_rotr64(b8, 8) + b0)
        x8 = _ladon_validate(x8 ^ r)
        temp.append(x8)
        x8 = _ladon_validate(x8 ^ _rotr64(b0, 61))
        ht[(r + 1) * 8:(r + 2) * 8] = x8.to_bytes(8, "little")
        b0 = x8
        b8 = temp.pop(0)
    ns = _ladon_pad_size(size)
    inp = bytearray(ns)
    inp[:size] = data
    _ladon_pad_buffer(inp, size, ns, 16)
    out = bytearray(ns)
    for i in range(ns // 16):
        out[i * 16:(i + 1) * 16] = _ladon_enc_input(ht, inp[i * 16:(i + 1) * 16])
    return bytes(out)


def ladon_encrypt(
    timestamp: int,
    license_id: int = 1611921764,
    aid: int = 1967,
    random_bytes: Optional[bytes] = None,
) -> str:
    rb = random_bytes if random_bytes is not None else os.urandom(4)
    data = f"{int(timestamp)}-{int(license_id)}-{int(aid)}".encode()
    keygen = rb + str(int(aid)).encode()
    md5hex = hashlib.md5(keygen).hexdigest().encode()
    output = rb + _ladon_encrypt_data(md5hex, data, len(data))
    return base64.b64encode(output).decode()


# =============== X-Gorgon (70932 variant) ===============
def _xg_reverse(num: int) -> int:
    tmp = hex(num)[2:]
    if len(tmp) < 2:
        tmp = '0' + tmp
    return int(tmp[1:] + tmp[:1], 16)


def _xg_rbit(num: int) -> int:
    tmp = bin(num)[2:]
    while len(tmp) < 8:
        tmp = '0' + tmp
    return int(tmp[::-1], 2)


def _xg_h2s(num: int) -> str:
    tmp = hex(num)[2:]
    return tmp if len(tmp) >= 2 else '0' + tmp


def gorgon_generate(params: str, stub_hex: str = "", timestamp: Optional[int] = None) -> str:
    if timestamp is None:
        timestamp = int(time.time())
    gorgon = []
    md5p = hashlib.md5(params.encode()).hexdigest()
    for i in range(4):
        gorgon.append(int(md5p[i * 2:i * 2 + 2], 16))
    if stub_hex and len(stub_hex) >= 8:
        for i in range(4):
            gorgon.append(int(stub_hex[i * 2:i * 2 + 2], 16))
    else:
        gorgon.extend([0] * 4)
    gorgon.extend([0] * 4)
    gorgon.extend([0] * 4)
    kh = f"{int(timestamp):08x}"
    for i in range(4):
        gorgon.append(int(kh[i * 2:i * 2 + 2], 16))

    tmp_val = ''
    hex_zu = list(range(256))
    for i in range(256):
        if i == 0:
            A = 0
        elif tmp_val:
            A = tmp_val
        else:
            A = hex_zu[i - 1]
        B = _XG_HEX_STR[i % 8]
        if A == 85 and i != 1 and tmp_val != 85:
            A = 0
        C = A + i + B
        while C >= 256:
            C -= 256
        if C < i:
            tmp_val = C
        else:
            tmp_val = ''
        hex_zu[i] = hex_zu[C]

    tmp_add = []
    tmp_hex = deepcopy(hex_zu)
    for i in range(_XG_LEN):
        A = gorgon[i]
        B = 0 if not tmp_add else tmp_add[-1]
        C = hex_zu[i + 1] + B
        while C >= 256:
            C -= 256
        tmp_add.append(C)
        D = tmp_hex[C]
        tmp_hex[i + 1] = D
        E = D + D
        while E >= 256:
            E -= 256
        gorgon[i] = A ^ tmp_hex[E]

    for i in range(_XG_LEN):
        A = gorgon[i]
        B = _xg_reverse(A)
        C = gorgon[(i + 1) % _XG_LEN]
        D = B ^ C
        E = _xg_rbit(D)
        F = E ^ _XG_LEN
        G = ~F
        while G < 0:
            G += 4294967296
        gorgon[i] = int(hex(G)[-2:], 16)

    result = ''.join(_xg_h2s(x) for x in gorgon)
    return f'0404{_xg_h2s(_XG_HEX_STR[7])}{_xg_h2s(_XG_HEX_STR[3])}{_xg_h2s(_XG_HEX_STR[1])}{_xg_h2s(_XG_HEX_STR[6])}{result}'


# =============== Top-level API ===============
def generate_registerkey_headers(params: str, stub_hex: str = "") -> dict:
    ts = int(time.time())
    headers = {
        "X-Gorgon": gorgon_generate(params, stub_hex=stub_hex, timestamp=ts),
        "X-Khronos": str(ts),
        "x-argus": argus_get_sign(params=params, stub=stub_hex, timestamp=ts, aid=1967, license_id=1611921764),
        "x-ladon": ladon_encrypt(timestamp=ts, license_id=1611921764, aid=1967),
    }
    if stub_hex:
        headers["x-ss-stub"] = stub_hex.upper()
    return headers
