import gzip
import hashlib
import json
import random
import time
import uuid
import base64
from typing import Optional

import httpx
from Crypto.Cipher import AES
from Crypto.Util.Padding import pad, unpad

from core.pure_sign import generate_all_headers
from core.device_pool import device_pool

_REGISTER_AES_KEY = b"\xac\x25\xc6\x7d\xdd\x8f\x38\xc1\xb3\x7a\x23\x48\x82\x8e\x22\x2e"
_MASTER_KEY = bytes.fromhex("ac25c67ddd8f38c1b37a2348828e222e")

UA = "com.dragon.read"

def _random_hex(n: int) -> str:
    return "".join(random.choices("0123456789abcdef", k=n))

def _random_model() -> str:
    chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    m = ""
    for i in range(8):
        if i == 3:
            m += "-"
        m += random.choice(chars)
    return m

def _reverse_hex(num_str: str) -> str:

    hex_str = format(int(num_str), "032x")
    result = ""
    for i in range(len(hex_str), 0, -2):
        result += hex_str[i - 2:i]
    return result

def _decrypt_secret_key(encrypted_key_b64: str) -> str:

    data = base64.b64decode(encrypted_key_b64)
    iv = data[:16]
    ciphertext = data[16:]
    cipher = AES.new(_MASTER_KEY[:16], AES.MODE_CBC, iv)
    plaintext = unpad(cipher.decrypt(ciphertext), 16)
    return plaintext.hex().upper()

async def device_register() -> dict:

    udid = _random_hex(16)
    model = _random_model()

    url = "https://i.snssdk.com/service/2/device_register/"
    body = {
        "header": {
            "package": "com.dragon.read",
            "openudid": udid,
            "device_model": model,
        }
    }

    async with httpx.AsyncClient(verify=False, timeout=15) as client:
        resp = await client.post(url, json=body)

    if resp.status_code != 200:
        raise Exception(f"设备注册失败: HTTP {resp.status_code}")

    resp_data = resp.json()
    device_id = str(resp_data.get("device_id", ""))
    install_id = str(resp_data.get("install_id", ""))

    if not device_id or device_id == "0":
        raise Exception("设备注册返回无效 device_id")
    if not install_id or install_id == "0":
        raise Exception("设备注册返回无效 install_id")

    return {
        "device_id": device_id,
        "install_id": install_id,
        "model": model,
    }

async def register_key(device_id: str, install_id: str, device_type: str = "Honor10") -> str:

    iv = uuid.uuid4().hex[:16].encode()
    hex_data = _reverse_hex(device_id)
    data = bytes.fromhex(hex_data)
    cipher = AES.new(_REGISTER_AES_KEY, AES.MODE_CBC, iv)
    encrypted = cipher.encrypt(pad(data, 16))
    content = base64.b64encode(iv + encrypted).decode()
    json_body = json.dumps({"content": content})

    post_body = gzip.compress(json_body.encode())

    all_params = (
        f"aid=1967&app_name=novelapp&channel=0&device_id={device_id}"
        f"&device_platform=android&device_type={device_type}"
        f"&iid={install_id}&os_version=0&version_code=99999"
    )
    param_parts = all_params.split("&")
    param_parts.sort()
    params = "&".join(param_parts)
    url = f"https://reading.snssdk.com/reading/crypt/registerkey?{params}"

    sign_headers = generate_all_headers(params, body_bytes=post_body, aid=1967)
    headers = {
        "User-Agent": UA,
        "Content-Type": "application/json",
        "Content-Encoding": "gzip",
        **sign_headers,
    }

    import asyncio as _aio
    last_err = ""
    for attempt in range(3):
        if attempt > 0:
            await _aio.sleep(1)

            sign_headers = generate_all_headers(params, body_bytes=post_body, aid=1967)
            headers = {
                "User-Agent": UA,
                "Content-Type": "application/json",
                "Content-Encoding": "gzip",
                **sign_headers,
            }

        async with httpx.AsyncClient(verify=False, timeout=15) as client:
            resp = await client.post(url, content=post_body, headers=headers)

        if resp.status_code != 200:
            last_err = f"HTTP {resp.status_code}"
            continue

        if not resp.text:
            last_err = "空响应"
            continue

        resp_data = resp.json()
        if resp_data.get("code") != 0:
            last_err = f"业务错误: {resp_data.get('message', resp_data)}"
            continue

        encrypted_key = resp_data.get("data", {}).get("key")
        if not encrypted_key:
            last_err = "未返回 key"
            continue

        return _decrypt_secret_key(encrypted_key)

    raise Exception(f"registerkey 失败 (重试3次): {last_err}")

async def register_device_and_key() -> dict:

    reg = await device_register()
    device_id = reg["device_id"]
    install_id = reg["install_id"]
    model = reg["model"]

    secret_key = await register_key(device_id, install_id, model)

    device_pool.add_device(device_id, install_id, secret_key)

    return {
        "device_id": device_id,
        "install_id": install_id,
        "secret_key": secret_key,
    }
