"""Captured Redfruit/Tomato feed protocol helpers without credentials or state."""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any
from urllib.parse import urlencode
from zoneinfo import ZoneInfo
from core.new_releases import series_comment_count


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


# IDs from the upstream cell_selector, not the public API board names.
RANK_TYPE_BOARDS = {
    "human": dict(zip(
        ["ranklist_hot_sc", "ranklist_hot_play_sc", "ranklist_new_rank_sc", "ranklist_hot_search_sc", "ranklist_must_watch", "ranklist_followed"],
        ["human_hot_sc", "human_hot_play", "human_new_rank", "human_hot_search", "human_must_watch", "human_followed"],
    )),
    "comic_series_rank": dict(zip(
        ["ranklist_hot_sc", "ranklist_hot_play_sc", "ranklist_new_rank_sc", "ranklist_hot_search_sc"],
        ["comic_series_hot_rank", "comic_series_hot_play", "comic_series_new_rank", "comic_series_hot_search"],
    )),
    "ai_playlet": dict(zip(
        ["ranklist_hot_sc", "ranklist_hot_play_sc", "ranklist_new_rank_sc", "ranklist_hot_search_sc", "ranklist_must_watch", "ranklist_followed"],
        ["ai_playlet_hot_sc", "ai_playlet_hot_play", "ai_playlet_new_rank", "ai_playlet_hot_search", "ai_playlet_must_watch", "ai_playlet_followed"],
    )),
}

def rank_selector(release_type: str, board: str) -> str:
    selector = "human" if release_type == "playlet" else release_type
    return selector if board in RANK_TYPE_BOARDS.get(selector, {}) else "all"


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
    if selected_items not in {"all", "human", "comic_series_rank", "ai_playlet", "playlet"}:
        raise ValueError(f"不支持的榜单类型: {selected_items}")
    if board not in RANK_BOARDS:
        raise ValueError(f"不支持的榜单: {board}")
    selected_items = rank_selector(selected_items, board)
    upstream_board = RANK_TYPE_BOARDS.get(selected_items, {}).get(board, board)
    following = state is not None
    params: dict[str, str] = {
        **_business_params(device_id),
        "tab_type": "26",
        "client_template": "2",
        "cell_id": RANK_CELL_ID,
        "selected_items": selected_items,
        "sub_selected_items": upstream_board,
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
            for label in re.split(r"[·,，、/|]+", value):
                label = label.strip()
                if (label and label not in result
                    and label not in {"短剧", "真人剧", "漫剧", "AI剧", "完结", "连载中"}
                    and not re.fullmatch(r"(?:(?:全|共|更新至|更新到|已更新|第)\s*)?\d+\s*(?:集|话|季)(?:全)?", label)):
                    result.append(label)
        elif isinstance(value, dict):
            if value.get("data_type") not in (None, 3):
                return
            add(value.get("name") or value.get("show_name") or value.get("title") or value.get("content"))
        elif isinstance(value, list):
            for child in value:
                add(child)
    for value in values:
        add(value)
    return result


def series_genres(video: dict, wrapper: dict | None = None) -> list[str]:
    wrapper = wrapper or {}
    # sub_title is genre · episode status · cast; later parts are not genres.
    subtitle = re.split(r"[·|]", str(video.get("sub_title") or ""))[0]
    rec_genres = [tag.get("content") for tag in video.get("rec_tags") or [] if isinstance(tag, dict) and tag.get("rec_type") == 24]
    return _category_tags(wrapper.get("sub_title_list"), wrapper.get("categories"),
                          wrapper.get("category"), [tag for tag in video.get("sub_title_list") or [] if isinstance(tag, dict) and tag.get("data_type") == 3], rec_genres, subtitle)


def series_release_type(video: dict) -> str:
    detail = video.get("video_detail") or {}
    if str(video.get("video_category_type") or detail.get("video_category_type") or "").lower() == "ai_video":
        return "ai_playlet"
    if int(video.get("content_type") or detail.get("content_type") or 0) in (2, 1004):
        return "comic_series_rank"
    return "playlet" if int(video.get("content_type") or detail.get("content_type") or 0) == 1 else ""


def text_metric(video: dict, label: str) -> int | None:
    texts = [(video.get("rec_text_item") or {}).get("RecommendText")]
    texts += [tag.get("content") for key in ("sub_title_list", "secondary_info_list")
              for tag in video.get(key) or [] if isinstance(tag, dict)]
    for text in texts:
        match = re.fullmatch(r"(?:🔥\s*)?(\d+(?:\.\d+)?)(万|亿)?(?:次)?" + label, str(text or "").strip())
        if match:
            return round(float(match[1]) * {None: 1, "万": 10000, "亿": 100000000}[match[2]])
    return None


def video_metric(video: dict, key: str, label: str) -> int | None:
    detail = video.get("video_detail") or {}
    for source in (video, detail):
        value = _optional_int(source.get(key))
        if value is not None:
            return value
    return text_metric(video, label)


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
    tags = series_genres(video, wrapper)
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
        "video_category_type": str(video.get("video_category_type") or ""),
        "duration": _optional_int(video.get("duration")) or 0,
        "abstract": video.get("video_desc") or detail.get("series_intro") or "",
        "score": video.get("score") or "",
        "category": " · ".join(tags),
        "category_tags": tags,
        "author": video.get("copyright") or "",
        "rank_tags": [],
        "release_type": release_type or series_release_type(video),
        "online_time": _optional_int(wrapper.get("schedule_publish_time")),
        "play_count": play_count,
        "hot_count": video_metric(video, "hot_score", "热度"),
        "collect_count": video_metric(video, "followed_cnt", "收藏"),
        "like_count": None,
        "comment_count": series_comment_count(video) if series_comment_count(video) is not None else series_comment_count(detail),
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
            "comment_count": series_comment_count(video),
        }
    return result
