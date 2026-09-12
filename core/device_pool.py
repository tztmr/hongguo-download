import json
import os
import time
from pathlib import Path
from threading import Lock
from typing import Optional

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DATA_ROOT = Path(os.environ["HONGGUO_DATA_DIR"]) if os.environ.get("HONGGUO_DATA_DIR") else PROJECT_ROOT / "config"
POOL_FILE = DATA_ROOT / "device_pool.json"

DEVICE_TTL = 86400

class DeviceEntry:

    def __init__(self, device_id: str, install_id: str, secret_key: str,
                 status: str = "active", fail_count: int = 0,
                 last_used: float = 0, last_fail: float = 0,
                 created_at: float = 0, update_time: str = ""):
        self.device_id = device_id
        self.install_id = install_id
        self.secret_key = secret_key
        self.status = status
        self.fail_count = fail_count
        self.last_used = last_used
        self.last_fail = last_fail
        self.created_at = created_at or time.time()
        self.update_time = update_time or time.strftime("%Y-%m-%d %H:%M:%S")

    def is_expired(self) -> bool:

        return (time.time() - self.created_at) > DEVICE_TTL

    def has_valid_identity(self) -> bool:
        return bool(
            self.device_id
            and self.device_id != "0"
            and self.install_id
            and self.install_id != "0"
            and self.secret_key
        )

    def to_dict(self) -> dict:
        return {
            "device_id": self.device_id,
            "install_id": self.install_id,
            "secret_key": self.secret_key,
            "status": self.status,
            "fail_count": self.fail_count,
            "last_used": self.last_used,
            "last_fail": self.last_fail,
            "created_at": self.created_at,
            "update_time": self.update_time,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "DeviceEntry":
        return cls(
            device_id=d["device_id"],
            install_id=d.get("install_id", ""),
            secret_key=d.get("secret_key", ""),
            status=d.get("status", "active"),
            fail_count=d.get("fail_count", 0),
            last_used=d.get("last_used", 0),
            last_fail=d.get("last_fail", 0),
            created_at=d.get("created_at", time.time()),
            update_time=d.get("update_time", ""),
        )

class DevicePool:

    MAX_FAIL_COUNT = 3
    FAIL_COOLDOWN = 600
    # 池硬上限: 失效转移连续注册新设备时防止无界膨胀, 超限淘汰最差
    MAX_POOL_SIZE = 30

    def __init__(self):
        self._lock = Lock()
        self._devices: list[DeviceEntry] = []
        self._load()
        self._cleanup_expired()

    def _load(self):
        if not POOL_FILE.exists():
            self._devices = []
            return
        try:
            data = json.loads(POOL_FILE.read_text(encoding="utf-8"))
            self._devices = [DeviceEntry.from_dict(d) for d in data.get("devices", [])]
        except Exception:
            self._devices = []

    def _save(self):
        POOL_FILE.parent.mkdir(parents=True, exist_ok=True)
        data = {"devices": [d.to_dict() for d in self._devices]}
        try:
            POOL_FILE.write_text(
                json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8"
            )
        except Exception:
            pass

    def _cleanup_expired(self):

        with self._lock:
            before = len(self._devices)
            self._devices = [d for d in self._devices if not d.is_expired()]
            removed = before - len(self._devices)
            if removed > 0:
                self._save()

    def get_best_device(self) -> Optional[DeviceEntry]:

        with self._lock:
            now = time.time()

            self._devices = [d for d in self._devices if not d.is_expired()]

            candidates = []
            for d in self._devices:
                if d.status == "active" and d.has_valid_identity():
                    candidates.append(d)
                elif (
                    d.status == "failed"
                    and d.has_valid_identity()
                    and (now - d.last_fail) > self.FAIL_COOLDOWN
                ):
                    d.status = "active"
                    d.fail_count = 0
                    candidates.append(d)

            if not candidates:
                return None

            best = min(candidates, key=lambda d: d.last_used)
            best.last_used = now
            self._save()
            return best

    def get_device(self, device_id: str) -> Optional[DeviceEntry]:
        """返回指定的可用设备，用于维持上游分页会话粘性。"""
        if not device_id:
            return None
        with self._lock:
            now = time.time()
            for device in self._devices:
                if device.device_id != device_id or device.is_expired():
                    continue
                if device.status == "failed":
                    if now - device.last_fail <= self.FAIL_COOLDOWN:
                        return None
                    device.status = "active"
                    device.fail_count = 0
                if device.status != "active" or not device.has_valid_identity():
                    return None
                device.last_used = now
                self._save()
                return device
        return None

    def report_success(self, device_id: str):
        with self._lock:
            for d in self._devices:
                if d.device_id == device_id:
                    d.fail_count = 0
                    d.status = "active"
                    break
            self._save()

    def report_failure(self, device_id: str):
        with self._lock:
            for d in self._devices:
                if d.device_id == device_id:
                    d.fail_count += 1
                    d.last_fail = time.time()
                    if d.fail_count >= self.MAX_FAIL_COUNT:
                        d.status = "failed"
                    break
            self._save()

    def add_device(self, device_id: str, install_id: str, secret_key: str) -> bool:
        with self._lock:

            for d in self._devices:
                if d.device_id == device_id:
                    d.secret_key = secret_key
                    d.install_id = install_id
                    d.status = "active"
                    d.fail_count = 0
                    d.created_at = time.time()
                    d.update_time = time.strftime("%Y-%m-%d %H:%M:%S")
                    self._save()
                    return True

            # 池满时淘汰最差设备: 失败多的 > 最久未用的, 保证池有界
            while len(self._devices) >= self.MAX_POOL_SIZE:
                worst = sorted(
                    self._devices,
                    key=lambda d: (d.status != "failed", d.fail_count, d.last_used),
                )[0]
                self._devices.remove(worst)

            entry = DeviceEntry(
                device_id=device_id,
                install_id=install_id,
                secret_key=secret_key,
            )
            self._devices.append(entry)
            self._save()
            return True

    def remove_device(self, device_id: str) -> bool:
        with self._lock:
            before = len(self._devices)
            self._devices = [d for d in self._devices if d.device_id != device_id]
            if len(self._devices) < before:
                self._save()
                return True
            return False

    def list_devices(self) -> list[dict]:
        with self._lock:
            now = time.time()
            result = []
            for d in self._devices:
                item = d.to_dict()
                item["remaining_seconds"] = max(0, int(DEVICE_TTL - (now - d.created_at)))
                item["expired"] = d.is_expired()
                result.append(item)
            return result

    def pool_size(self) -> int:
        with self._lock:
            return len(self._devices)

    def active_count(self) -> int:
        with self._lock:
            return sum(1 for d in self._devices if d.status == "active" and not d.is_expired())

    def save(self):
        with self._lock:
            self._save()

    def cleanup_expired(self):

        self._cleanup_expired()

device_pool = DevicePool()
