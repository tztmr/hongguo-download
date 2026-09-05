import hashlib
import time
from core.sign_70932 import gorgon_generate, argus_get_sign, ladon_encrypt


def generate_all_headers(
    params: str,
    body_bytes=None,
    aid: int = 1967,
    license_id: int = 1611921764,
    timestamp=None,
):
    if timestamp is None:
        timestamp = int(time.time())

    stub_hex = ""
    if body_bytes is not None:
        stub_hex = hashlib.md5(body_bytes).hexdigest().upper()

    headers = {
        "X-Gorgon": gorgon_generate(params, stub_hex=stub_hex, timestamp=timestamp),
        "X-Khronos": str(timestamp),
        "x-ladon": ladon_encrypt(timestamp=timestamp, license_id=license_id, aid=aid),
        "x-argus": argus_get_sign(params=params, stub=stub_hex, timestamp=timestamp, aid=aid, license_id=license_id),
    }
    if stub_hex:
        headers["x-ss-stub"] = stub_hex
    return headers
