import asyncio
import hashlib
import json
import logging
import random
import time
from typing import Callable, Optional
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse

import httpx

from core.pure_sign import generate_all_headers
from core.device_pool import device_pool, DeviceEntry
from core.device_register import register_device_and_key

logger = logging.getLogger("fanqie.client")
UA = "com.dragon.read"


def _with_device_identity(url: str, device: DeviceEntry) -> str:
    """Make the request identity match the device used for signing/failover."""

    parsed = urlparse(url)
    params = parse_qsl(parsed.query, keep_blank_values=True)
    identity = {
        "device_id": str(device.device_id),
        "iid": str(device.install_id),
    }
    for key, value in identity.items():
        params = [(existing_key, existing_value) for existing_key, existing_value in params if existing_key != key]
        params.append((key, value))
    return urlunparse(parsed._replace(query=urlencode(params)))


def _trigger_ensure_pool() -> None:
    """请求成功后异步补池到常备数量(调度器未启动时静默跳过)。"""

    try:
        from core.scheduler import scheduler
        scheduler.ensure_pool_soon()
    except Exception:
        pass

class PureSignedClient:

    # 单次业务调用的换设备重试轮数上限(每轮失败后自动注册新设备)
    FAILOVER_ROUNDS = 8
    # 并发注册新设备的信号量, 防止高并发失败时注册风暴
    REGISTER_SEMAPHORE = asyncio.Semaphore(3)

    def __init__(self, timeout: float = 15.0):
        self._client = httpx.AsyncClient(
            timeout=timeout,
            verify=False,
            follow_redirects=True,
            limits=httpx.Limits(
                max_connections=100,
                max_keepalive_connections=20,
            ),
        )

    async def close(self):
        await self._client.aclose()

    async def _register_fresh_device(self) -> Optional[DeviceEntry]:
        """注册一台全新设备并返回; 注册失败返回 None。"""

        async with PureSignedClient.REGISTER_SEMAPHORE:
            try:
                dev = await register_device_and_key()
            except Exception as e:
                logger.warning(f"注册新设备失败: {e}")
                return None
        return DeviceEntry(
            device_id=dev["device_id"],
            install_id=dev["install_id"],
            secret_key=dev["secret_key"],
        )

    async def _get_device_with_auto_register(
        self, preferred_device_id: str | None = None,
    ) -> DeviceEntry:

        if preferred_device_id:
            preferred = device_pool.get_device(preferred_device_id)
            if preferred and preferred.secret_key:
                return preferred

        entry = device_pool.get_best_device()
        if entry and entry.secret_key:
            return entry

        logger.info("设备池为空, 自动注册新设备...")
        dev = await register_device_and_key()
        entry = device_pool.get_best_device()
        if entry and entry.secret_key:
            return entry

        return DeviceEntry(
            device_id=dev["device_id"],
            install_id=dev["install_id"],
            secret_key=dev["secret_key"],
        )

    def _make_headers(self, query_string: str, body_bytes: Optional[bytes] = None,
                      aid: int = 1967) -> dict:

        sign_headers = generate_all_headers(query_string, body_bytes=body_bytes, aid=aid)
        ticket = str(int(time.time() * 1000))
        extra = {
            "x-ss-req-ticket": ticket,
            "x-reading-request": f"{int(ticket) + 1}-{random.randint(10000000, 99999999)}",
        }
        result = dict(sign_headers)
        result.update(extra)
        return result

    async def signed_get(
        self,
        url: str,
        aid: int = 1967,
        device: DeviceEntry | None = None,
    ) -> httpx.Response:

        if device is not None:
            url = _with_device_identity(url, device)
        parsed = urlparse(url)
        query_string = parsed.query or ""
        sign_headers = self._make_headers(query_string, aid=aid)
        headers = {"User-Agent": UA, **sign_headers}
        return await self._client.get(url, headers=headers)

    async def signed_post(self, url: str, data: str = "",
                          content_type: str = "application/x-www-form-urlencoded",
                          aid: int = 1967,
                          device: DeviceEntry | None = None) -> httpx.Response:

        if device is not None:
            url = _with_device_identity(url, device)
        parsed = urlparse(url)
        query_string = parsed.query or ""
        body_bytes = data.encode("utf-8") if isinstance(data, str) else data
        sign_headers = self._make_headers(query_string, body_bytes=body_bytes, aid=aid)
        headers = {
            "User-Agent": UA,
            "Content-Type": content_type,
            **sign_headers,
        }
        return await self._client.post(url, content=body_bytes, headers=headers)

    async def call_with_device(
        self,
        url_builder,
        method: str = "GET",
        data: str = "",
        aid: int = 1967,
        max_device_retries: int = 3,
        need_key: bool = False,
        content_type: str = "application/x-www-form-urlencoded",
        preferred_device_id: str | None = None,
        return_device_id: bool = False,
        validate_upstream: Callable[[dict], str | None] | None = None,
    ) -> dict:
        """带设备失效转移的请求: 上游失败时不向上抛错, 自动注册新设备重试。

        每轮失败 -> 标记设备失败 -> 立即注册全新设备 -> 下一轮顶上,
        直到成功或达到 FAILOVER_ROUNDS 轮上限; 成功后异步补池。
        """

        rounds = max(int(max_device_retries), PureSignedClient.FAILOVER_ROUNDS)
        last_err = ""
        tried_devices: set[str] = set()

        for attempt in range(rounds):

            # 1. 获取设备: 优先取池内, 取不到则直接注册新设备
            entry: Optional[DeviceEntry] = None
            try:
                entry = await self._get_device_with_auto_register(
                    preferred_device_id if attempt == 0 else None
                )
            except Exception as e:
                last_err = f"设备获取失败: {e}"
                logger.error(last_err)

            if entry is None or not entry.secret_key:
                entry = await self._register_fresh_device()
                if entry is None:
                    await asyncio.sleep(0.5)
                    continue

            # 2. 本轮设备已试过失败: 注册全新设备替换
            if entry.device_id in tried_devices:
                fresh = await self._register_fresh_device()
                if fresh is None:
                    await asyncio.sleep(0.5)
                    continue
                entry = fresh

            device_id = entry.device_id
            secret_key = entry.secret_key
            tried_devices.add(device_id)

            # 3. 发起请求
            url = url_builder(device_id)
            result = await self._do_call(
                url, method, data, aid, content_type=content_type, device=entry,
            )

            # A successful HTTP/business code can still omit data this device
            # is expected to support. Validate before reporting/caching success.
            if result["ok"] and validate_upstream is not None:
                validation_error = validate_upstream(result["upstream"])
                if validation_error:
                    result = {"ok": False, "msg": validation_error}

            if result["ok"]:
                device_pool.report_success(device_id)
                if need_key:
                    result["device_id"] = device_id
                    result["secret_key"] = secret_key
                elif return_device_id:
                    result["device_id"] = device_id
                # 后台补池, 不阻塞本次响应
                _trigger_ensure_pool()
                return result

            # 4. 失败: 标记 + 立即注册新设备供下一轮使用
            last_err = result["msg"]
            logger.warning(
                f"设备 {device_id} 请求失败 (attempt {attempt + 1}/{rounds}): {last_err}"
            )
            device_pool.report_failure(device_id)
            await self._register_fresh_device()
            await asyncio.sleep(0.2)

        return {"ok": False, "msg": f"换 {len(tried_devices)} 台设备重试 {rounds} 轮后仍失败: {last_err}"}

    async def _do_call(self, url: str, method: str, data: str, aid: int,
                       _retry: int = 1,
                       content_type: str = "application/x-www-form-urlencoded",
                       device: DeviceEntry | None = None) -> dict:

        last_err = ""
        for attempt in range(_retry + 1):
            try:
                if method.upper() == "POST":
                    resp = await self.signed_post(
                        url, data, content_type=content_type, aid=aid, device=device,
                    )
                else:
                    resp = await self.signed_get(url, aid=aid, device=device)

                if resp.status_code >= 500:
                    last_err = f"上游 HTTP {resp.status_code}"
                    if attempt < _retry:
                        await asyncio.sleep(0.5)
                        continue
                    return {"ok": False, "msg": last_err}

                if resp.status_code != 200:
                    return {"ok": False, "msg": f"上游 HTTP {resp.status_code}"}

                if not resp.text:
                    return {"ok": False, "msg": "上游返回空响应"}

                try:
                    upstream = resp.json()
                except json.JSONDecodeError:
                    return {"ok": False, "msg": "上游返回非 JSON"}

                if isinstance(upstream, dict) and upstream.get("code") not in (None, 0):
                    return {
                        "ok": False,
                        "msg": f"上游业务错误: {upstream.get('message') or upstream.get('msg', '未知')}",
                    }

                return {"ok": True, "upstream": upstream}

            except httpx.TimeoutException:
                last_err = "上游请求超时"
                if attempt < _retry:
                    await asyncio.sleep(0.5)
                    continue
            except Exception as e:
                last_err = f"请求异常: {str(e)}"
                if attempt < _retry:
                    await asyncio.sleep(0.5)
                    continue

        return {"ok": False, "msg": last_err}
