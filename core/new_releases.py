"""Pure helpers for normalized series metrics and Beijing-today release paging."""

from __future__ import annotations

import asyncio
import base64
import copy
import json
import secrets
import time
from collections import OrderedDict
from datetime import datetime
from typing import Any, Awaitable, Callable
from zoneinfo import ZoneInfo


SHANGHAI = ZoneInfo("Asia/Shanghai")
CURSOR_VERSION = 1
METADATA_CONCURRENCY = 4
_CURSOR_ERROR = "分页游标已失效"


def _walk(value: Any):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from _walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk(child)


def _optional_int(value: Any) -> int | None:
    if value is None or value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def normalize_metrics(upstream: dict, series_id: str) -> dict:
    """Extract stable metric fields without turning missing values into zero."""

    match = next(
        (
            item
            for item in _walk(upstream)
            if str(item.get("series_id") or item.get("series_id_str") or "")
            == str(series_id)
            and "create_time" in item
        ),
        {},
    )
    return {
        "online_time": _optional_int(match.get("create_time")),
        "play_count": _optional_int(match.get("series_play_cnt")),
        "hot_count": _optional_int(match.get("hot_score")),
        "collect_count": _optional_int(match.get("followed_cnt")),
        "like_count": _optional_int(match.get("digg_cnt")),
    }


def is_shanghai_today(timestamp: Any, now: datetime | None = None) -> bool:
    value = _optional_int(timestamp)
    if value is None or value <= 0:
        return False
    current = now or datetime.now(SHANGHAI)
    if current.tzinfo is None:
        current = current.replace(tzinfo=SHANGHAI)
    try:
        online_date = datetime.fromtimestamp(value, SHANGHAI).date()
    except (OSError, OverflowError, ValueError):
        return False
    return online_date == current.astimezone(SHANGHAI).date()


class CursorStore:
    """Bounded local state behind short opaque paging tokens."""

    def __init__(
        self,
        ttl_seconds: int = 600,
        max_entries: int = 256,
        clock: Callable[[], float] = time.monotonic,
    ):
        self.ttl_seconds = max(1, int(ttl_seconds))
        self.max_entries = max(1, int(max_entries))
        self.clock = clock
        self._entries: OrderedDict[str, tuple[float, str, str, dict]] = OrderedDict()

    def _prune(self) -> None:
        current = self.clock()
        expired = [key for key, entry in self._entries.items() if entry[0] < current]
        for key in expired:
            self._entries.pop(key, None)

    @staticmethod
    def _encode(payload: dict) -> str:
        raw = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")
        return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")

    @staticmethod
    def _decode(token: str) -> dict:
        try:
            padding = "=" * (-len(token) % 4)
            value = json.loads(base64.urlsafe_b64decode(token + padding).decode("utf-8"))
        except (ValueError, UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise ValueError(_CURSOR_ERROR) from exc
        if not isinstance(value, dict):
            raise ValueError(_CURSOR_ERROR)
        return value

    def put(self, release_type: str, date: str, state: dict) -> str:
        self._prune()
        while len(self._entries) >= self.max_entries:
            self._entries.popitem(last=False)
        key = secrets.token_urlsafe(16)
        expires_at = self.clock() + self.ttl_seconds
        self._entries[key] = (
            expires_at,
            release_type,
            date,
            copy.deepcopy(state),
        )
        return self._encode(
            {"version": CURSOR_VERSION, "type": release_type, "date": date, "key": key}
        )

    def get(self, token: str, expected_type: str, expected_date: str) -> dict:
        self._prune()
        payload = self._decode(token)
        if (
            payload.get("version") != CURSOR_VERSION
            or payload.get("type") != expected_type
            or payload.get("date") != expected_date
            or not isinstance(payload.get("key"), str)
        ):
            raise ValueError(_CURSOR_ERROR)
        entry = self._entries.get(payload["key"])
        if entry is None:
            raise ValueError(_CURSOR_ERROR)
        expires_at, release_type, date, state = entry
        if (
            expires_at < self.clock()
            or release_type != expected_type
            or date != expected_date
        ):
            self._entries.pop(payload["key"], None)
            raise ValueError(_CURSOR_ERROR)
        self._entries.move_to_end(payload["key"])
        return copy.deepcopy(state)


def _series_id(item: dict) -> str:
    return str(item.get("series_id") or item.get("book_id") or "")


def _state_marker(value: Any) -> str:
    try:
        return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    except (TypeError, ValueError):
        return repr(value)


async def collect_today_releases(
    *,
    fetch_page: Callable[[Any], Awaitable[dict]],
    fetch_metrics: Callable[[dict], Awaitable[dict]],
    release_type: str,
    cursor: str = "",
    limit: int = 20,
    now: datetime | None = None,
    cursor_store: CursorStore,
    only_today: bool = True,
) -> dict:
    """Collect one visible group while retaining over-fetched matching rows.

    The live-action subscribe feed has a reliable daily schedule, while the
    comic/AI new-release rank only exposes its latest ranked rows. Callers can
    disable the calendar filter for the latter feed without changing cursor
    handling or de-duplication.
    """

    if limit < 1 or limit > 20:
        raise ValueError("limit must be between 1 and 20")
    current = now or datetime.now(SHANGHAI)
    if current.tzinfo is None:
        current = current.replace(tzinfo=SHANGHAI)
    current = current.astimezone(SHANGHAI)
    date = current.date().isoformat()
    state = (
        cursor_store.get(cursor, release_type, date)
        if cursor
        else {"upstream": None, "has_more": True, "seen": [], "buffer": []}
    )
    upstream_state = state.get("upstream")
    upstream_has_more = bool(state.get("has_more", True))
    seen = {str(value) for value in state.get("seen") or [] if value}
    matches = [item for item in state.get("buffer") or [] if isinstance(item, dict)]
    visited_states = {_state_marker(upstream_state)}
    semaphore = asyncio.Semaphore(METADATA_CONCURRENCY)

    async def enrich(item: dict) -> dict:
        async with semaphore:
            try:
                metrics = await fetch_metrics(item)
            except Exception:
                metrics = {
                    "online_time": item.get("online_time"),
                    "play_count": None,
                    "hot_count": None,
                    "collect_count": None,
                    "like_count": None,
                }
            return {**item, **metrics}

    while len(matches) < limit and upstream_has_more:
        page = await fetch_page(upstream_state)
        upstream_state = page.get("next")
        upstream_has_more = bool(page.get("has_more"))
        next_marker = _state_marker(upstream_state)
        if upstream_has_more and next_marker in visited_states:
            raise RuntimeError("上游分页状态未前进")
        visited_states.add(next_marker)
        unique = []
        for item in page.get("items") or []:
            if not isinstance(item, dict):
                continue
            series_id = _series_id(item)
            if not series_id or series_id in seen:
                continue
            seen.add(series_id)
            unique.append({**item, "series_id": series_id})
        if unique:
            enriched = await asyncio.gather(*(enrich(item) for item in unique))
            if only_today:
                matches.extend(
                    item for item in enriched
                    if is_shanghai_today(item.get("online_time"), current)
                )
            else:
                matches.extend(enriched)

    matches.sort(
        key=lambda item: (_optional_int(item.get("online_time")) or 0, _series_id(item)),
        reverse=True,
    )
    items = matches[:limit]
    buffer = matches[limit:]
    has_more = bool(buffer or upstream_has_more)
    next_cursor = ""
    if has_more:
        next_cursor = cursor_store.put(
            release_type,
            date,
            {
                "upstream": upstream_state,
                "has_more": upstream_has_more,
                "seen": list(seen),
                "buffer": buffer,
            },
        )
    return {
        "items": items,
        "next_cursor": next_cursor,
        "has_more": has_more,
        "date": date,
        "refreshed_at": current.isoformat(),
    }
