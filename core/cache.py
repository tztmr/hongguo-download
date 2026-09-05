import time
from threading import Lock
from typing import Any, Optional

class TTLCache:

    def __init__(self, default_ttl: int = 300, max_size: int = 2048):
        self._store: dict[str, tuple[float, Any]] = {}
        self._lock = Lock()
        self._default_ttl = default_ttl
        self._max_size = max_size

    def get(self, key: str) -> Optional[Any]:
        with self._lock:
            entry = self._store.get(key)
            if entry is None:
                return None
            expire_at, value = entry
            if time.monotonic() > expire_at:
                del self._store[key]
                return None
            return value

    def set(self, key: str, value: Any, ttl: Optional[int] = None):
        ttl = ttl if ttl is not None else self._default_ttl
        with self._lock:
            if len(self._store) >= self._max_size:
                self._evict()
            self._store[key] = (time.monotonic() + ttl, value)

    def _evict(self):
        now = time.monotonic()
        expired = [k for k, (exp, _) in self._store.items() if now > exp]
        for k in expired:
            del self._store[k]
        if len(self._store) >= self._max_size:
            sorted_keys = sorted(self._store, key=lambda k: self._store[k][0])
            for k in sorted_keys[: len(sorted_keys) // 2]:
                del self._store[k]

    def clear(self):
        with self._lock:
            self._store.clear()

def make_cache_key(prefix: str, **kwargs) -> str:

    parts = [prefix]
    for k in sorted(kwargs.keys()):
        parts.append(f"{k}={kwargs[k]}")
    return "|".join(parts)
