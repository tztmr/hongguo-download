import struct
import unittest
from Crypto.Cipher import AES
from core.mp4_decrypt import aes_ctr_decrypt


class CounterTests(unittest.TestCase):
    def test_nist_ctr_vector(self):
        key = bytes.fromhex('2b7e151628aed2a6abf7158809cf4f3c')
        iv = bytes.fromhex('f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff')
        encrypted = bytes.fromhex('874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff')
        plain = bytes.fromhex('6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51')
        self.assertEqual(aes_ctr_decrypt(encrypted, key, iv), plain)

    def test_low_64_bits_wrap_without_carry_and_partial_samples(self):
        key = bytes(range(16))
        for low in [0, 2**64-2, 2**64-1]:
            iv = struct.pack('>QQ', 987654321, low)
            for size in [0, 1, 15, 16, 17, 49, 1025]:
                data = bytes(i % 251 for i in range(size))
                cipher = AES.new(key, AES.MODE_ECB)
                stream = b''.join(cipher.encrypt(struct.pack('>QQ', 987654321, (low+i) % 2**64)) for i in range((size+15)//16))
                expected = bytes(a ^ b for a, b in zip(data, stream))
                self.assertEqual(aes_ctr_decrypt(data, key, iv), expected)

    def test_invalid_key_and_iv_rejected_even_for_empty_data(self):
        for key, iv in [(bytes(15), bytes(16)), (bytes(16), bytes(8))]:
            with self.assertRaises(ValueError):
                aes_ctr_decrypt(b'', key, iv)
