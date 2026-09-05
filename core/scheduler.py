import asyncio
import logging

from core.device_pool import device_pool
from core.device_register import register_device_and_key

logger = logging.getLogger("fanqie.scheduler")

# 常备可用设备数: 低于该值自动注册补充
MIN_ACTIVE_DEVICES = 10
# 保活巡检间隔(秒)
ENSURE_INTERVAL = 300
# 过期清理间隔(秒)
CLEANUP_INTERVAL = 600
# 批量注册时每台之间的间隔(秒), 避免注册接口压力
REGISTER_STAGGER = 0.5


class DeviceScheduler:

    def __init__(self):
        self._tasks: list[asyncio.Task] = []
        self._running = False
        self._ensure_lock = asyncio.Lock()
        self._bg_task: asyncio.Task | None = None

    async def start(self):

        if self._running:
            return
        self._running = True
        self._tasks = [
            asyncio.create_task(self._ensure_pool_loop(), name="ensure_pool"),
            asyncio.create_task(self._cleanup_loop(), name="cleanup"),
        ]
        logger.info(
            "设备保活任务已启动 (常备 %d 台, 每 %d 秒巡检补齐)",
            MIN_ACTIVE_DEVICES, ENSURE_INTERVAL,
        )

    async def stop(self):

        self._running = False
        for task in self._tasks:
            task.cancel()
        for task in self._tasks:
            try:
                await task
            except asyncio.CancelledError:
                pass
        if self._bg_task and not self._bg_task.done():
            self._bg_task.cancel()
            try:
                await self._bg_task
            except asyncio.CancelledError:
                pass
        self._tasks = []

    async def ensure_pool(self, min_count: int = MIN_ACTIVE_DEVICES) -> int:
        """将活跃设备补齐到 min_count, 返回本次新注册数量。并发安全。"""

        async with self._ensure_lock:
            need = min_count - device_pool.active_count()
            if need <= 0:
                return 0
            registered = 0
            for _ in range(need):
                try:
                    await register_device_and_key()
                    registered += 1
                except Exception as e:
                    logger.warning(f"补池注册失败: {e}")
                    await asyncio.sleep(1)
                else:
                    await asyncio.sleep(REGISTER_STAGGER)
            if registered:
                logger.info(
                    "设备池已补齐: 新增 %d, 当前活跃 %d/%d",
                    registered, device_pool.active_count(), device_pool.pool_size(),
                )
            return registered

    def ensure_pool_soon(self) -> None:
        """非阻塞触发一次补池; 已有补池任务运行时自动去重。"""

        if not self._running:
            return
        if self._bg_task and not self._bg_task.done():
            return
        try:
            loop = asyncio.get_running_loop()
            self._bg_task = loop.create_task(self.ensure_pool())
        except RuntimeError:
            pass

    async def _ensure_pool_loop(self):

        while self._running:
            try:
                await self.ensure_pool()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"保活巡检异常: {e}")
            await asyncio.sleep(ENSURE_INTERVAL)

    async def _cleanup_loop(self):

        while self._running:
            try:
                await asyncio.sleep(CLEANUP_INTERVAL)
                if not self._running:
                    break
                device_pool.cleanup_expired()
                # 清理后立即补齐, 避免出现空窗
                await self.ensure_pool()
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error(f"清理异常: {e}")

    async def register_now(self, count: int = 1) -> dict:

        results = []
        for _ in range(count):
            try:
                dev = await register_device_and_key()
                results.append({
                    "device_id": dev["device_id"],
                    "status": "active",
                })
            except Exception as e:
                results.append({"error": str(e)})
            await asyncio.sleep(REGISTER_STAGGER)
        return {
            "registered": len([r for r in results if "error" not in r]),
            "devices": results,
            "pool_size": device_pool.pool_size(),
            "active_count": device_pool.active_count(),
        }


scheduler = DeviceScheduler()
