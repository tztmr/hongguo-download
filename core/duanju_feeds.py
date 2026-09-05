"""Captured Redfruit/Tomato feed protocol helpers without credentials or state."""

from __future__ import annotations

from datetime import datetime
from typing import Any
from urllib.parse import urlencode
from zoneinfo import ZoneInfo


FEED_API = "https://api5-normal-sinfonlineb.fqnovel.com"
SUBSCRIBE_PATH = "/reading/user/subscribe/list/v:version/"
RANK_PATH = "/reading/bookapi/bookmall/cell/change/v:version/"
RANK_CELL_ID = "7470092475068071998"
SHANGHAI = ZoneInfo("Asia/Shanghai")

TODAY_RELEASE_TYPES = {
    "playlet": "真人剧",
    "comic_series_rank": "漫剧",
    "ai_playlet": "AI剧",
}

RANK_BOARDS = {
    "ranklist_hot_sc": "推荐榜",
    "ranklist_hot_play_sc": "热播榜",
    "ranklist_prestige": "臻果榜",
    "ranklist_subscribe": "预约榜",
    "ranklist_new_rank_sc": "新剧榜",
    "ranklist_hot_search_sc": "热搜榜",
    "ranklist_must_watch": "必看榜",
    "ranklist_followed": "收藏榜",
}


def _business_params(device_id: str) -> dict[str, str]:
    return {
        "aid": "8662",
        "app_name": "novelread",
        "version_name": "7.3.2.32",
        "version_code": "732",
        "update_version_code": "73232",
        "device_platform": "iphone",
        "device_id": str(device_id),
    }


def build_subscribe_url(
    device_id: str,
    target_date: str,
    *,
    offset: int = 0,
    session_id: str = "",
) -> str:
    params = {
        **_business_params(device_id),
        "target_date": target_date,
        "tab_type": "5",
        "active_panel": "5",
        "filter_type": "gender",
        "gender_type": "2",
        "order_experiment": "descend",
        "offset": str(max(0, int(offset))),
        "session_id": session_id,
    }
    return f"{FEED_API}{SUBSCRIBE_PATH}?{urlencode(params)}"


def build_rank_url(
    device_id: str,
    selected_items: str,
    board: str,
    *,
    state: dict[str, Any] | None = None,
) -> str:
    if selected_items not in {"all", "comic_series_rank", "ai_playlet", "playlet"}:
        raise ValueError(f"不支持的榜单类型: {selected_items}")
    if board not in RANK_BOARDS:
        raise ValueError(f"不支持的榜单: {board}")
    following = state is not None
    params: dict[str, str] = {
        **_business_params(device_id),
        "tab_type": "26",
        "client_template": "2",
        "cell_id": RANK_CELL_ID,
        "selected_items": selected_items,
        "sub_selected_items": board,
        "panel_selected_items": "",
        "client_req_type": "2",
        "unlimited_selector_change_type": "1" if following else "2",
        "rank_version": "",
    }
    if following:
        assert state is not None
        params.update(
            {
                "offset": str(max(0, int(state.get("offset") or 0))),
                "session_id": str(state.get("session_id") or ""),
                "rank_version": str(state.get("rank_version") or ""),
                "filter_ids": ",".join(
                    str(value) for value in (state.get("filter_ids") or []) if value
                ),
            }
        )
    return f"{FEED_API}{RANK_PATH}?{urlencode(params)}"


def _optional_int(value: Any) -> int | None:
    if value is None or value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _category_tags(*values: Any) -> list[str]:
    result: list[str] = []

    def add(value: Any) -> None:
        if isinstance(value, str):
            label = value.strip()
            if label and label not in result:
                result.append(label)
        elif isinstance(value, dict):
            add(value.get("name") or value.get("show_name") or value.get("title"))
        elif isinstance(value, list):
            for child in value:
                add(child)

    for value in values:
        add(value)
    return result


def _normalize_video(
    video: dict[str, Any],
    release_type: str | None,
    *,
    wrapper: dict[str, Any] | None = None,
) -> dict[str, Any]:
    wrapper = wrapper or {}
    detail = video.get("video_detail") or {}
    series_id = str(
        video.get("series_id")
        or detail.get("series_id")
        or detail.get("series_id_str")
        or ""
    )
    tags = _category_tags(
        wrapper.get("sub_title_list"),
        wrapper.get("categories"),
        wrapper.get("category"),
        video.get("sub_title"),
    )
    play_count = _optional_int(detail.get("series_play_cnt"))
    if play_count is None:
        play_count = _optional_int(video.get("play_cnt"))
    return {
        "series_id": series_id,
        "book_id": series_id,
        "title": video.get("title") or detail.get("series_title") or "",
        "cover": video.get("cover") or detail.get("series_cover") or "",
        "first_vid": str(video.get("vid") or detail.get("first_vid") or ""),
        "episode_count": _optional_int(
            video.get("episode_cnt") or detail.get("episode_cnt")
        )
        or 0,
        "content_type": _optional_int(
            video.get("content_type") or detail.get("content_type")
        )
        or 1,
        "duration": _optional_int(video.get("duration")) or 0,
        "abstract": video.get("video_desc") or detail.get("series_intro") or "",
        "score": video.get("score") or "",
        "category": " · ".join(tags),
        "category_tags": tags,
        "author": video.get("copyright") or "",
        "rank_tags": [],
        "release_type": release_type or "",
        "online_time": _optional_int(wrapper.get("schedule_publish_time")),
        "play_count": play_count,
        "hot_count": None,
        "collect_count": _optional_int(detail.get("followed_cnt")),
        "like_count": None,
    }


def _is_target_date(timestamp: Any, target_date: str) -> bool:
    value = _optional_int(timestamp)
    if value is None or value <= 0:
        return False
    try:
        return datetime.fromtimestamp(value, SHANGHAI).strftime("%Y%m%d") == target_date
    except (OSError, OverflowError, ValueError):
        return False


def parse_subscribe_page(upstream: dict[str, Any], target_date: str) -> dict[str, Any]:
    data = upstream.get("data") or {}
    items = []
    seen: set[str] = set()
    for row in data.get("subscribe_items") or []:
        if not isinstance(row, dict) or row.get("is_online") is not True:
            continue
        if not _is_target_date(row.get("schedule_publish_time"), target_date):
            continue
        video = row.get("subscribe_data") or {}
        if not isinstance(video, dict):
            continue
        item = _normalize_video(video, "playlet", wrapper=row)
        series_id = item["series_id"]
        if not series_id or item["content_type"] != 1 or series_id in seen:
            continue
        seen.add(series_id)
        items.append(item)
    return {
        "items": items,
        "next_offset": int(data.get("next_offset") or 0),
        "session_id": str(data.get("session_id") or ""),
        "rank_version": "",
        "has_more": bool(data.get("has_more")),
    }


def _video_rows(value: Any):
    if isinstance(value, dict):
        videos = value.get("video_data")
        if isinstance(videos, list):
            for video in videos:
                if isinstance(video, dict):
                    yield video
        for child in value.values():
            if isinstance(child, (dict, list)):
                yield from _video_rows(child)
    elif isinstance(value, list):
        for child in value:
            yield from _video_rows(child)


def parse_rank_page(
    upstream: dict[str, Any], *, release_type: str | None = None
) -> dict[str, Any]:
    data = upstream.get("data") or {}
    items = []
    seen: set[str] = set()
    for video in _video_rows(data.get("cell_view") or {}):
        item = _normalize_video(video, release_type)
        series_id = item["series_id"]
        if not series_id or series_id in seen:
            continue
        seen.add(series_id)
        items.append(item)
    return {
        "items": items,
        "next_offset": int(data.get("next_offset") or 0),
        "session_id": str(data.get("session_id") or ""),
        "rank_version": str(data.get("rank_version") or ""),
        "has_more": bool(data.get("has_more")),
    }


def parse_batch_metrics(upstream: dict[str, Any]) -> dict[str, dict[str, int | None]]:
    result: dict[str, dict[str, int | None]] = {}
    data = upstream.get("data") or {}
    if not isinstance(data, dict):
        return result
    for key, value in data.items():
        if not isinstance(value, dict):
            continue
        video = value.get("video_data") or {}
        if not isinstance(video, dict):
            continue
        series_id = str(video.get("series_id") or key or "")
        if not series_id:
            continue
        result[series_id] = {
            "online_time": _optional_int(video.get("create_time")),
            "play_count": _optional_int(video.get("series_play_cnt")),
            "hot_count": _optional_int(video.get("hot_score")),
            "collect_count": _optional_int(video.get("followed_cnt")),
            "like_count": _optional_int(video.get("digg_cnt")),
        }
    return result
