"""番茄短剧接口,复用项目现有 70932 纯算签名与设备池。"""
import asyncio
import base64
import copy
import json
import logging
import time
from datetime import datetime, timedelta
from typing import Annotated
from typing import Any
from urllib.parse import quote

import httpx
from fastapi import APIRouter, Query, Request
from fastapi.responses import Response, StreamingResponse

from core.cache import TTLCache, make_cache_key
from core.duanju_feeds import (
    FEED_API,
    RANK_BOARDS as CAPTURED_RANK_BOARDS,
    TODAY_RELEASE_TYPES,
    build_rank_url,
    build_subscribe_url,
    parse_batch_metrics,
    rank_selector,
    _normalize_video,
    parse_rank_page as parse_captured_rank_page,
    parse_subscribe_page,
)
from core.playback import prepare_compatible_video, prepare_streaming_video, while_connected
from core.mp4_decrypt import decrypt_mp4, derive_key_from_spade_a
from core.video_download import create_video_client, download_video
from core.new_releases import (
    SHANGHAI,
    CursorStore,
    collect_today_releases,
    normalize_metrics,
)
from core.response import error, success
from core.download_diagnostics import record_failure

logger = logging.getLogger('fanqie.duanju')
router = APIRouter()

BASE_API = 'https://api5-normal-sinfonlinea.fqnovel.com'
VIDEO_URL = 'https://reading.snssdk.com/novel/player/multi_video_model/v1/'
VIDEO_QUERY = 'aid=8662&device_platform=android&update_version_code=70132'
DRAMA_TAB_TYPE = '11'
MANJU_TAB_TYPE = '19'
# 剧场栏 bookmall 发现页: 番茄畅读 7.2.3.32 (aid=8662 novelread)
BOOKMALL_API = 'https://api5-normal-sinfonlinec.fqnovel.com'
DRAMA_BOOKMALL_TAB = '38'   # BookstoreTabType.real_person_series
MANJU_BOOKMALL_TAB = '32'   # BookstoreTabType.comic_series
RANK_TAB_TYPE = '26'
RANK_CELL_ID = '7470092475068071998'
NEW_RELEASE_CELL_ID = '7431550523368554558'
NEW_RELEASE_URL = f'{BOOKMALL_API}/reading/bookapi/bookmall/cell/change/v1/'
SERIES_METADATA_URL = f'{BOOKMALL_API}/novel/player/multi_video_detail/preload/v1/'
CAPTURED_METADATA_URL = f'{FEED_API}/novel/player/multi_video_detail/preload/v1/'
RANK_GROUPS = {
    'all': '总榜',
    'playlet': '短剧',
    'comic_series_rank': '漫剧',
    'ai_playlet': 'AI剧',
}
RANK_CONTENT_TYPE_CODES = {
    'playlet': 1,
    'comic_series_rank': 1004,
}
AI_VIDEO_CATEGORY_TYPE = 'ai_video'
RANK_BOARDS = dict(CAPTURED_RANK_BOARDS)

_duanju_search_cache = TTLCache(default_ttl=300)
_duanju_detail_cache = TTLCache(default_ttl=3600)
_duanju_catalog_cache = TTLCache(default_ttl=3600)
_duanju_discovery_cache = TTLCache(default_ttl=1800)
_duanju_video_cache = TTLCache(default_ttl=300)
_duanju_series_metadata_cache = TTLCache(default_ttl=300)
_duanju_new_release_cache = TTLCache(default_ttl=60)
_duanju_rank_page_cache = TTLCache(default_ttl=60, max_size=512)
_duanju_rank_page_inflight: dict[str, asyncio.Task] = {}
_duanju_new_release_cursors = CursorStore(ttl_seconds=600, max_entries=256)
_duanju_rank_cursors = CursorStore(ttl_seconds=600, max_entries=512)
_duanju_rank_sessions = TTLCache(default_ttl=1800, max_size=512)


def _business_params(device_id: str) -> str:
    """旧版画像 66.9: 短剧搜索/详情/目录只返 content_type=1, 不混漫剧。"""
    return (
        f'aid=1967&app_name=novelapp&channel=0&device_platform=android'
        f'&device_id={device_id}&device_type=Honor10&os_version=0'
        f'&version_code=66.9&update_version_code=58932'
    )


def _business_params_v70132(device_id: str) -> str:
    """新版画像 70132: 漫剧 tab19 与发现页 landing 只在该画像下可用。"""
    return (
        f'aid=1967&app_name=novelapp&channel=0&device_platform=android'
        f'&device_id={device_id}&device_type=Honor10&os_version=0'
        f'&version_code=70132&version_name=7.0.1.32'
        f'&update_version_code=70132&manifest_version_code=70132'
    )


def _rank_business_params(device_id: str) -> str:
    """排行榜/上新 cell 抓包对应的红果 iOS 画像。"""
    return (
        f'aid=1967&app_name=novelapp&device_platform=iphone'
        f'&device_id={device_id}&version_code=733&update_version_code=73332'
    )


def _bookmall_params(device_id: str) -> str:
    """剧场栏 bookmall 画像: 番茄畅读 7.2.3.32 (aid=8662 novelread)。"""
    return (
        f'aid=8662&app_name=novelread&version_code=72932&version_name=7.2.9.32'
        f'&device_platform=android&os=android&device_type=PLQ110&device_brand=OnePlus'
        f'&os_api=36&os_version=16&update_version_code=72932&manifest_version_code=72932'
        f'&device_id={device_id}'
    )


def _series_item(video: dict) -> dict:
    """从 bookmall video_data 抽取统一短剧字段。"""
    detail = video.get('video_detail') or {}
    rank_tags = []
    for tag in video.get('rec_tags') or []:
        if not isinstance(tag, dict):
            continue
        content = str(tag.get('content') or '')
        schema = str(tag.get('schema') or '')
        if '榜' not in content and 'mainRank' not in schema:
            continue
        rank_tags.append({
            'label': content,
            'schema': schema,
        })
    return {**_normalize_video(video, None), 'rank_tags': rank_tags}


def _series_metadata_body(series_id: str, content_type: int) -> str:
    return json.dumps({
        'series_id': str(series_id),
        'content_type': int(content_type),
        'biz_param': {
            'use_os_player': False,
            'device_level': 3,
            'source': 0,
            'disable_video_relate_book': False,
            'screen_width_px': '828',
            'disable_digg_stat': False,
            'detail_page_version': 0,
            'need_mp4_align': False,
            'video_platform': 0,
            'need_all_video_definition': False,
            'video_id_type': 1,
            'use_server_dns': False,
        },
    }, ensure_ascii=False, separators=(',', ':'))


def _series_metadata_batch_body(series_ids: list[str]) -> str:
    return _series_metadata_body(','.join(series_ids), 0)


def _new_release_candidates(upstream: dict) -> list[dict]:
    """Extract normalized candidates from the nested bookmall cell response."""

    items: list[dict] = []
    seen: set[str] = set()

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            videos = value.get('video_data')
            if isinstance(videos, list):
                for video in videos:
                    if not isinstance(video, dict):
                        continue
                    item = _series_item(video)
                    series_id = item['series_id']
                    if not series_id or series_id in seen:
                        continue
                    seen.add(series_id)
                    if video.get('create_time') is not None:
                        item['online_time'] = video.get('create_time')
                    items.append(item)
            for child in value.values():
                if isinstance(child, (dict, list)):
                    visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    visit(upstream.get('data') or upstream)
    return items


async def _fetch_series_metrics(client, series_id: str, content_type: int) -> dict:
    content_type = int(content_type or 1)
    cache_key = make_cache_key(
        'duanju_series_metrics', series_id=series_id, content_type=content_type,
    )
    cached = _duanju_series_metadata_cache.get(cache_key)
    if cached is not None:
        return cached
    body = _series_metadata_body(series_id, content_type)

    def build_url(device_id: str) -> str:
        query = (
            f'series_id={quote(series_id, safe="")}&content_type={content_type}'
            f'&device_level=3&need_all_video_definition=false&{_bookmall_params(device_id)}'
        )
        return f'{SERIES_METADATA_URL}?{query}'

    result = await client.call_with_device(
        build_url,
        method='POST',
        data=body,
        aid=8662,
        max_device_retries=3,
        content_type='application/json',
    )
    if not result['ok']:
        raise RuntimeError(result['msg'])
    metrics = normalize_metrics(result['upstream'], series_id)
    _duanju_series_metadata_cache.set(cache_key, metrics)
    return metrics


async def _fetch_series_metrics_batch(
    client, series_ids: list[str], preferred_device_id: str = '',
) -> dict[str, dict]:
    unique_ids = list(dict.fromkeys(str(value) for value in series_ids if value))
    if not unique_ids:
        return {}
    joined = ','.join(unique_ids)
    body = _series_metadata_batch_body(unique_ids)

    def build_url(device_id: str) -> str:
        query = (
            f'series_id={quote(joined, safe=",")}&content_type=0&device_level=3'
            f'&need_all_video_definition=false&aid=8662&app_name=novelread'
            f'&version_name=7.3.2.32&version_code=732&update_version_code=73232'
            f'&device_platform=iphone&device_id={quote(device_id, safe="")}'
        )
        return f'{CAPTURED_METADATA_URL}?{query}'

    result = await client.call_with_device(
        build_url,
        method='POST',
        data=body,
        aid=8662,
        max_device_retries=3,
        content_type='application/json',
        preferred_device_id=preferred_device_id or None,
    )
    if not result['ok']:
        raise RuntimeError(result['msg'])
    return parse_batch_metrics(result['upstream'])


async def _fetch_rank_feed_page(
    client,
    selector_type: str,
    board: str,
    state: dict | None,
    *,
    target_date: str,
    preferred_device_id: str = '',
) -> dict:
    """Cache normalized upstream pages and share simultaneous identical reads."""
    cache_key = make_cache_key(
        'duanju_rank_page', selector_type=selector_type, board=board,
        state=json.dumps(state, sort_keys=True, separators=(',', ':')),
        target_date=target_date, device_id=preferred_device_id,
    )
    cached = _duanju_rank_page_cache.get(cache_key)
    if cached is not None:
        return copy.deepcopy(cached)

    async def fetch() -> dict:
        result = await client.call_with_device(
            lambda device_id: build_rank_url(device_id, selector_type, board, state=state),
            aid=8662, max_device_retries=3,
            preferred_device_id=preferred_device_id or None,
            return_device_id=True,
        )
        if not result['ok']:
            raise RuntimeError(result['msg'])
        page = {
            **parse_captured_rank_page(result['upstream']),
            'device_id': str(result.get('device_id') or preferred_device_id),
        }
        # A transient broken cursor must remain retryable. The caller still
        # decides whether to fall back to another selector or reject the page.
        if not page['has_more'] or page['next_offset'] > int((state or {}).get('offset') or 0):
            _duanju_rank_page_cache.set(cache_key, page)
        return page

    task = _duanju_rank_page_inflight.get(cache_key)
    if task is None:
        task = asyncio.create_task(fetch())
        _duanju_rank_page_inflight[cache_key] = task

        def finish(completed: asyncio.Task) -> None:
            if _duanju_rank_page_inflight.get(cache_key) is completed:
                _duanju_rank_page_inflight.pop(cache_key, None)
            if not completed.cancelled():
                completed.exception()

        task.add_done_callback(finish)
    # One disconnected caller must not cancel a request shared by other callers.
    return copy.deepcopy(await asyncio.shield(task))


async def _fetch_new_release_page(
    client,
    release_type: str,
    state: Any,
    *,
    target_date: str | None = None,
) -> dict:
    if release_type not in TODAY_RELEASE_TYPES:
        raise ValueError(f'不支持的监听类型: {release_type}')
    if isinstance(state, dict):
        current_offset = int(state.get('offset') or 0)
        session_id = str(state.get('session_id') or '')
        rank_version = str(state.get('rank_version') or '')
        preferred_device_id = str(state.get('device_id') or '')
        stored_filter_ids = state.get('filter_ids') or []
        if isinstance(stored_filter_ids, str):
            seen_ids = [value for value in stored_filter_ids.split(',') if value]
        else:
            seen_ids = [str(value) for value in stored_filter_ids if value]
        selector_type = str(state.get('selector_type') or release_type)
    else:
        current_offset = int(state or 0)
        session_id = ''
        rank_version = ''
        preferred_device_id = ''
        seen_ids = []
        selector_type = release_type
    current_date = target_date or datetime.now(SHANGHAI).strftime('%Y%m%d')
    cache_key = make_cache_key(
        'duanju_new_releases', release_type=release_type, offset=current_offset,
        session_id=session_id, rank_version=rank_version,
        filter_ids=','.join(seen_ids[-200:]), target_date=current_date,
        selector_type=selector_type, device_id=preferred_device_id,
    )
    cached = _duanju_new_release_cache.get(cache_key)
    if cached is not None:
        return copy.deepcopy(cached)

    if release_type == 'playlet':
        result = await client.call_with_device(
            lambda device_id: build_subscribe_url(
                device_id, current_date, offset=current_offset, session_id=session_id,
            ),
            aid=8662, max_device_retries=3,
            preferred_device_id=preferred_device_id or None,
            return_device_id=True,
        )
        if not result['ok']:
            raise RuntimeError(result['msg'])
        preferred_device_id = str(result.get('device_id') or preferred_device_id)
        parsed = parse_subscribe_page(result['upstream'], current_date)
    else:
        upstream_state = None
        if current_offset or session_id or rank_version or seen_ids:
            upstream_state = {
                'offset': current_offset,
                'session_id': session_id,
                'rank_version': rank_version,
                'filter_ids': seen_ids[-200:],
            }
        parsed = await _fetch_rank_feed_page(
            client, selector_type, 'ranklist_new_rank_sc', upstream_state,
            target_date=current_date, preferred_device_id=preferred_device_id,
        )
        preferred_device_id = parsed['device_id']
        raw_items = parsed['items']
        parsed['items'] = _filter_release_items(raw_items, release_type)
        if (
            selector_type != 'all'
            and (not parsed['items'] or len(parsed['items']) != len(raw_items))
        ):
            # 新剧榜的类型 selector 也可能失效，改用混合榜后再按类型隔离。
            selector_type = 'all'
            # Different selectors have different sessions and offsets.
            current_offset, session_id, rank_version = 0, '', ''
            seen_ids = []
            parsed = await _fetch_rank_feed_page(
                client, selector_type, 'ranklist_new_rank_sc', None,
                target_date=current_date, preferred_device_id=preferred_device_id,
            )
            preferred_device_id = parsed['device_id']
            parsed['items'] = _filter_release_items(parsed['items'], release_type)
        try:
            metrics = await _fetch_series_metrics_batch(
                client, [item['series_id'] for item in parsed['items']],
                preferred_device_id,
            )
        except Exception:
            logger.warning('新剧指标补充暂不可用，保留榜单已知信息')
            metrics = {}
        parsed['items'] = [
            {**item, **{key: value for key, value in metrics.get(item['series_id'], {}).items() if value is not None}, 'release_type': release_type}
            for item in parsed['items']
        ]
    items = parsed['items']
    if parsed.get('has_more') and int(parsed.get('next_offset') or 0) <= current_offset:
        repeated_terminal_page = (
            release_type == 'playlet'
            and current_offset > 0
            and bool(seen_ids)
        )
        if repeated_terminal_page:
            parsed['has_more'] = False
        else:
            raise RuntimeError('上游分页 offset 未前进')
    next_filter_ids = [*seen_ids]
    known = set(next_filter_ids)
    for item in items:
        series_id = str(item.get('series_id') or '')
        if series_id and series_id not in known:
            known.add(series_id)
            next_filter_ids.append(series_id)
    next_state = {
        'offset': int(parsed.get('next_offset') or 0),
        'session_id': str(parsed.get('session_id') or session_id),
        'rank_version': str(parsed.get('rank_version') or rank_version),
        'filter_ids': next_filter_ids[-200:],
        'device_id': preferred_device_id,
    }
    if release_type != 'playlet':
        next_state['selector_type'] = selector_type
    page = {
        'items': items,
        'next': next_state,
        'has_more': bool(parsed.get('has_more')),
    }
    _duanju_new_release_cache.set(cache_key, copy.deepcopy(page))
    return page


def _parse_bookmall_tab(upstream: dict, tab_type: str) -> dict:
    """解析 bookmall/tab/v 首屏: 定位目标 tab 的推荐 cell 并提取翻页状态。"""
    data = upstream.get('data') or {}
    result = {
        'items': [], 'cell_id': '', 'next_offset': 0, 'has_more': True,
        'session_id': '',
        'plan_id': '',
        'tab_list': [],
        'categories': [],
    }
    for tab in data.get('tab_item') or []:
        result['tab_list'].append({'tab_type': tab.get('tab_type'), 'title': tab.get('title') or ''})
        if str(tab.get('tab_type')) != tab_type:
            continue
        # bookstore_id/session_id/has_more 位于 tab_item 层级(每个 tab 独立)
        result['session_id'] = str(tab.get('session_id') or '')
        result['plan_id'] = str(tab.get('bookstore_id') or '')
        result['has_more'] = bool(tab.get('has_more', True))
        for cell in tab.get('cell_data') or []:
            items = [_series_item(v) for sub in (cell.get('cell_data') or [])
                     for v in (sub.get('video_data') or []) if v]
            if not items:
                continue
            result['items'] = items
            result['cell_id'] = str(cell.get('cell_id') or cell.get('cell_id_str') or '')
            result['next_offset'] = int(cell.get('next_offset') or 0)
            result['has_more'] = bool(cell.get('has_more', result['has_more']))
            result['categories'] = _selector_categories(cell.get('cell_selector') or {})
            break
        break
    return result


def _parse_bookmall_change(upstream: dict) -> dict:
    """解析 bookmall/cell/change/v 响应: 新一页内容与轮换后的分页状态。"""
    data = upstream.get('data') or {}
    cell = data.get('cell_view') or {}
    items = [_series_item(v) for sub in (cell.get('cell_data') or [])
             for v in (sub.get('video_data') or []) if v]
    return {
        'items': items,
        'next_offset': int(data.get('next_offset') or 0),
        'has_more': bool(data.get('has_more')),
        'session_id': data.get('session_id') or '',
        'cell_id': str(cell.get('cell_id') or cell.get('cell_id_str') or ''),
        'boards': _selector_categories(cell.get('cell_selector') or {}),
    }


def _merge_filter_ids(existing: str, items: list[dict]) -> str:
    """合并已展示 series_id 去重队列(客户端上限 200, 滚动淘汰)。"""
    queue = [sid for sid in existing.split(',') if sid]
    for item in items:
        sid = str(item.get('series_id') or '')
        if sid and sid not in queue:
            queue.append(sid)
    return ','.join(queue[-200:])


def _selector_categories(selector: dict) -> list[dict]:
    """从 bookmall cell_selector 抽出可点分类。"""
    seen: set[str] = set()
    categories: list[dict] = []
    rows = []
    outer = selector.get('outer_row')
    if isinstance(outer, dict):
        rows.append(outer)
    rows.extend(row for row in (selector.get('inner_rows') or []) if isinstance(row, dict))
    for row in rows:
        group = str(row.get('row_name') or '')
        for item in row.get('items') or []:
            if not isinstance(item, dict):
                continue
            cid = str(item.get('selector_item_id') or '')
            name = str(item.get('show_name') or '')
            if not cid or not name or cid in seen:
                continue
            seen.add(cid)
            categories.append({
                'id': cid,
                'name': name,
                'group': group,
            })
    return categories


def _category_groups(selector: dict) -> list[dict]:
    """Normalize selector rows while preserving upstream group/item order."""

    rows: list[dict] = []
    outer = selector.get('outer_row')
    if isinstance(outer, dict):
        rows.append(outer)
    rows.extend(
        row for row in (selector.get('inner_rows') or []) if isinstance(row, dict)
    )
    groups: list[dict] = []
    seen: set[str] = set()
    for index, row in enumerate(rows):
        name = str(row.get('row_name') or ('综合' if index == 0 else '')).strip()
        if not name:
            continue
        items = []
        for item in row.get('items') or []:
            if not isinstance(item, dict):
                continue
            item_id = str(item.get('selector_item_id') or '').strip()
            item_name = str(item.get('show_name') or '').strip()
            if not item_id or not item_name or item_id in seen:
                continue
            seen.add(item_id)
            items.append({'id': item_id, 'name': item_name})
        if items:
            groups.append({'id': name, 'name': name, 'items': items})
    return groups


def _filter_release_items(items: list[dict], release_type: str) -> list[dict]:
    """用上游类型字段二次隔离真人剧、漫剧和 AI 剧，修正混榜响应。"""
    is_ai = lambda item: str(item.get('video_category_type') or '').lower() == AI_VIDEO_CATEGORY_TYPE
    if release_type == 'ai_playlet':
        return [item for item in items if is_ai(item)]
    expected = RANK_CONTENT_TYPE_CODES.get(release_type)
    if expected is None:
        return items
    return [
        item for item in items
        if int(item.get('content_type') or 0) == expected and not is_ai(item)
    ]


def _bookmall_selector(upstream: dict, tab_type: str) -> dict:
    for tab in (upstream.get('data') or {}).get('tab_item') or []:
        if not isinstance(tab, dict) or str(tab.get('tab_type')) != str(tab_type):
            continue
        for cell in tab.get('cell_data') or []:
            if isinstance(cell, dict) and isinstance(cell.get('cell_selector'), dict):
                return cell['cell_selector']
    return {}


def _split_rank_selector(selector: dict) -> tuple[list[dict], list[dict]]:
    """榜单 tab 走 outer_row, 分类筛选项走 inner_rows / cate_ 前缀。"""
    boards: list[dict] = []
    categories: list[dict] = []
    seen_board: set[str] = set()
    seen_cat: set[str] = set()
    outer = selector.get('outer_row') if isinstance(selector.get('outer_row'), dict) else {}
    for item in (outer.get('items') or []):
        if not isinstance(item, dict):
            continue
        cid = str(item.get('selector_item_id') or '')
        name = str(item.get('show_name') or '')
        if not cid or not name or cid in seen_board:
            continue
        seen_board.add(cid)
        boards.append({'id': cid, 'name': name, 'group': '榜单'})
    for row in selector.get('inner_rows') or []:
        if not isinstance(row, dict):
            continue
        group = str(row.get('row_name') or '')
        for item in row.get('items') or []:
            if not isinstance(item, dict):
                continue
            cid = str(item.get('selector_item_id') or '')
            name = str(item.get('show_name') or '')
            if not cid or not name or cid in seen_cat:
                continue
            seen_cat.add(cid)
            categories.append({'id': cid, 'name': name, 'group': group})
    return boards, categories


def _rank_boards(items: list[dict]) -> list[dict]:
    """从剧目 rank_tags 归纳发现页榜单角标。完整榜单页走 /duanju/rank。"""
    boards: list[dict] = []
    seen: set[str] = set()
    for item in items:
        for tag in item.get('rank_tags') or []:
            schema = str(tag.get('schema') or '')
            label = str(tag.get('label') or '')
            if 'mainRank' not in schema:
                continue
            key = schema
            if key in seen:
                continue
            seen.add(key)
            boards.append({
                'label': label.split(' No.')[0] if ' No.' in label else label,
                'schema': schema,
            })
    return boards


def _parse_rank_page(upstream: dict) -> dict:
    parsed = _parse_bookmall_change(upstream)
    data = upstream.get('data') or {}
    cell = data.get('cell_view') or {}
    boards, categories = _split_rank_selector(cell.get('cell_selector') or {})
    if not boards:
        boards = [{'id': key, 'name': name, 'group': '榜单'} for key, name in RANK_BOARDS.items()]
    return {
        'items': parsed['items'],
        'cell_id': parsed.get('cell_id') or RANK_CELL_ID,
        'next_offset': parsed['next_offset'],
        'has_more': parsed['has_more'],
        'session_id': parsed['session_id'],
        'boards': boards,
        'categories': categories,
        'groups': [{'id': key, 'name': name} for key, name in RANK_GROUPS.items()],
    }


async def _rank_fallback_categories(request: Request) -> list[dict]:
    """排行榜 cell 常不带 inner 分类, 回落到发现页真人剧题材条。"""
    cache_key = make_cache_key('duanju_discovery_v3', tab_type=DRAMA_BOOKMALL_TAB, filter_ids='', selected_items='')
    cached = _duanju_discovery_cache.get(cache_key)
    if isinstance(cached, dict) and cached.get('categories'):
        return cached['categories']

    def build_url(device_id: str) -> str:
        base = (
            f'tab_version=double_col&client_template=12'
            f'&bottom_tab_type_list=7%2C0%2C1%2C5%2C2%2C4'
            f'&tab_type={DRAMA_BOOKMALL_TAB}&book_id=0&landing_bottom_tab_type=7&bottom_tab_type=0'
            f'&client_req_type=1&offset=0&device_level=3'
        )
        return f'{BOOKMALL_API}/reading/bookapi/bookmall/tab/v?{base}&{_bookmall_params(device_id)}'

    result = await request.app.state.client.call_with_device(build_url, aid=8662, max_device_retries=2)
    if not result['ok']:
        return []
    parsed = _parse_bookmall_tab(result['upstream'], DRAMA_BOOKMALL_TAB)
    return parsed.get('categories') or []


def _search_items(upstream: dict, tab_type: str) -> tuple[list[dict], bool, int, str]:
    for tab in upstream.get('search_tabs') or []:
        if str(tab.get('tab_type')) != tab_type:
            continue
        rows = tab.get('data') or []
        items: list[dict] = []
        seen: set[str] = set()
        for row in rows:
            videos = row.get('video_data') or []
            if not videos:
                continue
            video = videos[0] or {}
            series_id = str(video.get('series_id') or row.get('book_id') or '')
            if not series_id or series_id in seen:
                continue
            seen.add(series_id)
            detail = video.get('video_detail') or {}
            items.append({
                'book_id': series_id,
                'title': video.get('title') or detail.get('series_title') or row.get('cell_name') or '',
                'cover': video.get('cover') or detail.get('series_cover') or '',
                'first_vid': str(video.get('vid') or detail.get('first_vid') or ''),
                'episode_count': video.get('episode_cnt') or detail.get('episode_cnt') or 0,
                'content_type': video.get('content_type') or 1,
                'score': video.get('score') or '',
                'rec_text': video.get('rec_text') or '',
                'category': video.get('sub_title') or '',
                'abstract': detail.get('series_intro') or '',
                'author': video.get('copyright') or '',
            })
        next_offset = int(tab.get('next_offset') or 0)
        next_passback = tab.get('passback')
        if next_passback is None:
            next_passback = tab.get('next_passback')
        if next_passback is None:
            next_passback = str(next_offset) if next_offset > 0 else ''
        return items, bool(tab.get('has_more')), next_offset, str(next_passback)
    return [], False, 0, ''


def _episode_list(upstream: dict) -> list[dict]:
    rows = (upstream.get('data') or {}).get('item_data_list') or []
    episodes = []
    for index, item in enumerate(rows, start=1):
        item_id = str(item.get('item_id') or '')
        if item_id:
            episodes.append({
                'index': index,
                'item_id': item_id,
                'title': item.get('title') or f'第{index}集',
            })
    return episodes


def _decode_url(value: Any) -> str:
    if not isinstance(value, str) or not value:
        return ''
    if value.startswith(('http://', 'https://')):
        return value
    try:
        decoded = base64.b64decode(value).decode('utf-8')
        return decoded if decoded.startswith(('http://', 'https://')) else ''
    except (ValueError, UnicodeDecodeError):
        return ''


def _video_sources(upstream: dict, vid: str) -> list[dict]:
    data = upstream.get('data') or {}
    node = data.get(str(vid)) or next((value for value in data.values() if isinstance(value, dict) and value.get('video_model')), {})
    model = node.get('video_model') if isinstance(node, dict) else ''
    if isinstance(model, str):
        try:
            model = json.loads(model)
        except json.JSONDecodeError:
            model = {}
    video_list = model.get('video_list') if isinstance(model, dict) else None
    pairs = video_list.items() if isinstance(video_list, dict) else enumerate(video_list or [])
    sources = []
    for key, item in pairs:
        if not isinstance(item, dict):
            continue
        meta = item.get('video_meta') if isinstance(item.get('video_meta'), dict) else item
        encrypt_info = item.get('encrypt_info') if isinstance(item.get('encrypt_info'), dict) else {}
        backups = item.get('backup_url')
        if not isinstance(backups, list):
            backups = [backups]
        urls = [_decode_url(item.get('main_url'))]
        urls.extend(_decode_url(value) for value in backups)
        urls.extend(_decode_url(item.get(f'backup_url_{index}')) for index in range(1, 4))
        urls = list(dict.fromkeys(url for url in urls if url))
        sources.append({
            'definition': meta.get('definition') or item.get('definition') or str(key),
            'codec_type': meta.get('codec_type') or item.get('codec_type') or '',
            'width': meta.get('vwidth') or item.get('vwidth') or 0,
            'height': meta.get('vheight') or item.get('vheight') or 0,
            'duration': model.get('video_duration', 0) if isinstance(model, dict) else 0,
            'size': meta.get('size') or item.get('size') or 0,
            'urls': urls,
            'spade_a': encrypt_info.get('spade_a') or item.get('spade_a') or meta.get('spade_a') or '',
        })
    return sources


@router.get('/duanju/search')
async def duanju_search(
    request: Request,
    key: str = Query(..., min_length=1, description='短剧搜索关键词'),
    offset: int = Query(0, ge=0, description='分页偏移'),
    passback: str = Query('', description='番茄搜索分页状态,使用上一页响应返回值'),
    content_type: str = Query('drama', pattern='^(drama|manju)$', description='drama=短剧,manju=漫剧'),
):
    is_manju = content_type == 'manju'
    tab_type = MANJU_TAB_TYPE if is_manju else DRAMA_TAB_TYPE
    if offset == 0:
        passback = ''
    cache_key = make_cache_key('duanju_search', key=key, offset=offset, passback=passback, tab_type=tab_type)
    cached = _duanju_search_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    encoded_key = quote(key, safe='')
    # 漫剧 tab19 只在 70132 画像下存在, 短剧 tab11 用 66.9 避免混入漫剧
    params = _business_params_v70132 if is_manju else _business_params

    def build_url(device_id: str) -> str:
        # 番茄搜索使用 offset + passback 双状态分页; 首页 passback 保持空值。
        return (
            f'{BASE_API}/reading/bookapi/search/tab/v/?{params(device_id)}'
            f'&tab_type={tab_type}&query={encoded_key}&offset={offset}'
            f'&passback={quote(passback, safe="")}'
        )

    def validate_search_tab(upstream: dict) -> str | None:
        tabs = upstream.get('search_tabs') if isinstance(upstream, dict) else None
        if isinstance(tabs, list) and any(
            isinstance(tab, dict) and str(tab.get('tab_type')) == tab_type for tab in tabs
        ):
            return None
        return f'当前设备未返回{"漫剧" if is_manju else "真人剧"}搜索分类，请重试或缩短关键词'

    result = await request.app.state.client.call_with_device(
        build_url, max_device_retries=3, validate_upstream=validate_search_tab,
    )
    if not result['ok']:
        logger.error('短剧搜索失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    items, has_more, next_offset, next_passback = _search_items(result['upstream'], tab_type)
    data = {
        'items': items,
        'has_more': has_more,
        'next_offset': next_offset,
        'next_passback': next_passback,
        'raw': result['upstream'],
    }
    _duanju_search_cache.set(cache_key, data)
    return success(data)


@router.get('/duanju/detail')
async def duanju_detail(request: Request, book_id: str = Query(..., min_length=1, description='短剧 series_id')):
    cache_key = make_cache_key('duanju_detail', book_id=book_id)
    cached = _duanju_detail_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        return f'https://reading.snssdk.com/reading/bookapi/detail/v/?{_business_params(device_id)}&book_id={quote(book_id, safe="")}'

    result = await request.app.state.client.call_with_device(build_url, max_device_retries=3)
    if not result['ok']:
        logger.error('短剧详情失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    data = result['upstream']
    _duanju_detail_cache.set(cache_key, data)
    return success(data)


@router.get('/duanju/series-metrics')
async def duanju_series_metrics(
    request: Request,
    series_id: str = Query(..., min_length=1, description='剧目 series_id'),
    content_type: int = Query(1, ge=1, description='上游数字 content_type'),
):
    try:
        metrics = await _fetch_series_metrics(
            request.app.state.client, series_id, content_type,
        )
    except RuntimeError as exc:
        logger.warning('剧目指标请求失败: %s', exc)
        return error(str(exc), code=-3, status_code=502)
    return success({
        'series_id': series_id,
        'content_type': content_type,
        **metrics,
    })


@router.get('/duanju/new-releases')
async def duanju_new_releases(
    request: Request,
    release_type: str = Query(
        'playlet', alias='type', pattern='^(playlet|comic_series_rank|ai_playlet)$',
    ),
    cursor: str = Query('', description='后端生成的短游标'),
    limit: int = Query(20, ge=1, le=20),
    days: Annotated[int | None, Query(ge=1, le=30, description='含今天，最近多少个北京时间自然日')] = None,
):
    client = request.app.state.client
    if release_type not in TODAY_RELEASE_TYPES:
        return error(f'不支持的监听类型: {release_type}', code=-1, status_code=400)
    current = datetime.now(SHANGHAI)
    target_date = current.strftime('%Y%m%d')

    async def fetch_page(offset):
        if release_type == 'playlet' and days is not None:
            state = offset or {}
            day = int(state.get('calendar_day', 0))
            page = await _fetch_new_release_page(client, release_type, state.get('page'),
                target_date=(current - timedelta(days=day)).strftime('%Y%m%d'))
            if page.get('has_more'):
                return {**page, 'next': {'calendar_day': day, 'page': page.get('next')}}
            return {**page, 'has_more': day + 1 < days, 'next': {'calendar_day': day + 1, 'page': None}}
        return await _fetch_new_release_page(
            client, release_type, offset, target_date=target_date,
        )

    async def fetch_metrics(item):
        return {
            'online_time': item.get('online_time'),
            'play_count': item.get('play_count'),
            'hot_count': item.get('hot_count'),
            'collect_count': item.get('collect_count'),
            'like_count': item.get('like_count'),
            'comment_count': item.get('comment_count'),
        }

    try:
        data = await collect_today_releases(
            fetch_page=fetch_page,
            fetch_metrics=fetch_metrics,
            release_type=release_type,
            cursor=cursor,
            limit=limit,
            now=current,
            cursor_store=_duanju_new_release_cursors,
            only_today=release_type == 'playlet',
            days=days,
        )
        data['source'] = 'subscribe' if release_type == 'playlet' else 'rank'
        data['date_scope'] = 'recent' if days is not None else 'today' if release_type == 'playlet' else 'latest'
        data['days'] = days
    except ValueError as exc:
        return error(str(exc), code=-2, status_code=400)
    except RuntimeError as exc:
        logger.warning('今日新剧候选请求失败: %s', exc)
        return error(str(exc), code=-3, status_code=502)
    return success(data)


@router.get('/duanju/catalog')
async def duanju_catalog(request: Request, book_id: str = Query(..., min_length=1, description='短剧 series_id')):
    cache_key = make_cache_key('duanju_catalog', book_id=book_id)
    cached = _duanju_catalog_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        return f'{BASE_API}/reading/bookapi/directory/all_items/v/?{_business_params(device_id)}&book_id={quote(book_id, safe="")}'

    result = await request.app.state.client.call_with_device(build_url, max_device_retries=3)
    if not result['ok']:
        logger.error('短剧目录失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    data = {'items': _episode_list(result['upstream']), 'raw': result['upstream']}
    _duanju_catalog_cache.set(cache_key, data)
    return success(data)


async def _fetch_video_model(request: Request, item_id: str) -> dict:
    """请求 multi_video_model 并缓存标准化播放源。"""
    cache_key = make_cache_key('duanju_content', item_id=item_id)
    cached = _duanju_video_cache.get(cache_key)
    if cached is not None:
        return cached

    body = json.dumps({
        'biz_param': {'detail_page_version': 0, 'device_level': 3, 'need_all_video_definition': True, 'need_mp4_align': True, 'use_os_player': True, 'video_platform': 1024},
        'mixed_video_id_map': {'1': [item_id]},
    }, separators=(',', ':')).encode('utf-8')
    url = f'{VIDEO_URL}?{VIDEO_QUERY}'
    result = await request.app.state.client.call_with_device(
        lambda _device_id: url,
        method='POST',
        data=body,
        aid=8662,
        max_device_retries=3,
        content_type='application/json',
    )
    if not result['ok']:
        raise RuntimeError(result['msg'])
    upstream = result['upstream']
    data = {'item_id': item_id, 'sources': _video_sources(upstream, item_id), 'raw': upstream}
    if not data['sources']:
        raise RuntimeError('播放模型响应中没有可用视频源')
    _duanju_video_cache.set(cache_key, data)
    return data


_DOWNLOAD_CODECS = frozenset({
    'h264', 'avc', 'avc1', 'avc3', 'h265', 'hevc', 'hvc1', 'hev1', 'bytevc1',
    'av1', 'av01', 'vp9', 'vp09',
})


def _pick_source(sources: list[dict], definition: str, prefer_h264: bool = False,
                 compatible_only: bool = False) -> dict:
    """按目标档位挑选播放源: 命中优先, 否则先降档再升档。"""
    tiers = ['1080p', '720p', '540p', '480p', '360p']
    usable = [item for item in sources if item['urls'] and item['spade_a']]
    if not usable:
        raise RuntimeError('没有同时具备播放地址和 spade_a 的视频源')
    if compatible_only:
        # ByteVC2 (bvc2) MP4s can have valid dimensions/duration and still have
        # no decoder in our FFmpeg. Filter before quality fallback or download,
        # rather than retrying identical, structurally complete media forever.
        usable = [item for item in usable
                  if str(item.get('codec_type', '')).strip().lower().replace('.', '')
                  in _DOWNLOAD_CODECS | {''}]
        if not usable:
            raise RuntimeError('当前剧集没有可下载合并的兼容编码视频源；等待源站提供 H.264 / HEVC 等版本后重试')
    if prefer_h264:
        # Keep requested quality; prefer AVC only among sources at the same tier.
        usable.sort(key=lambda item: str(item.get('codec_type', '')).lower() not in ('h264', 'avc', 'avc1', 'avc3'))
    want = definition.strip().lower()
    if want == 'auto':
        order = tiers
    else:
        index = tiers.index(want) if want in tiers else tiers.index('720p')
        order = tiers[index:] + tiers[index - 1::-1] if index else tiers
    for tier in order:
        for item in usable:
            if str(item['definition']).lower() == tier:
                return item
    return usable[0]


@router.get('/duanju/content')
async def duanju_content(request: Request, item_id: str = Query(..., min_length=1, description='剧集 item_id / vid')):
    """获取剧集播放模型;返回加密 CDN 地址和 spade_a。"""
    try:
        return success(await _fetch_video_model(request, item_id))
    except RuntimeError as exc:
        logger.warning('短剧播放模型失败: %s', exc)
        return error(str(exc), code=-3, status_code=502)


@router.get('/duanju/key')
async def duanju_key(
    request: Request,
    item_id: str = Query(..., min_length=1, description='剧集 item_id / vid'),
    definition: str = Query('720p', description='目标清晰度,命中不到时自动降/升档'),
):
    """派生剧集 CENC AES 密钥,用于外部播放器自行解密。"""
    try:
        data = await _fetch_video_model(request, item_id)
        source = _pick_source(data['sources'], definition)
        key_hex = derive_key_from_spade_a(source['spade_a'])
    except (RuntimeError, ValueError) as exc:
        logger.warning('短剧密钥派生失败: %s', exc)
        return error(str(exc), code=-5, status_code=502)
    return success({
        'item_id': item_id,
        'definition': source['definition'],
        'key_hex': key_hex,
        'encryption_method': 'cenc-aes-ctr',
        'urls': source['urls'],
        'size': source['size'],
    })


async def _download_encrypted(urls: list[str], client=None) -> bytes:
    if client is not None:
        return await download_video(urls, client)
    async with create_video_client() as temporary_client:
        return await download_video(urls, temporary_client)


@router.get('/duanju/download')
async def duanju_download(
    request: Request,
    item_id: str = Query(..., min_length=1, description='剧集 item_id / vid'),
    definition: str = Query('720p', description='目标清晰度,命中不到时自动降/升档'),
    playback_compat: bool = Query(False, description='仅在线观看: 转为 H.264/AAC 兼容 MP4'),
    playback_stream: bool = Query(False, description='仅兼容播放: 边转换边播放'),
):
    # A ten-series media group must not allocate ten encrypted/decrypted MP4s
    # simultaneously. Waiting requests use no video buffers or AI/GPU slots.
    if not hasattr(request.app.state, 'download_slots'):
        request.app.state.download_slots = asyncio.Semaphore(3)
    slots = request.app.state.download_slots
    try:
        await asyncio.wait_for(slots.acquire(), timeout=20)
    except asyncio.TimeoutError:
        response = error('下载服务繁忙，等待可用内存与下载名额后自动重试', code=-11, status_code=503)
        response.headers['Retry-After'] = '30'
        return response
    try:
        started = time.monotonic()
        for attempt in range(2):
            request.state.download_stage = '播放地址解析'
            try:
                response = await _duanju_download_once(request, item_id, definition, playback_compat, playback_stream)
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                stage = request.state.download_stage
                reference = record_failure(item_id, stage, exc)
                if isinstance(exc, MemoryError):
                    return error(f'下载内存不足，等待资源释放后自动恢复（记录 {reference}）', code=-11, status_code=503)
                response = error(f'{stage}异常：{type(exc).__name__}；将自动恢复（记录 {reference}）', code=-12, status_code=502)
            if response.status_code < 500 or playback_compat:
                return response
            # Retry an early transient error once with fresh playback URLs. Do
            # not cache failures, change quality, or restart completed episodes.
            _duanju_video_cache.delete(make_cache_key('duanju_content', item_id=item_id))
            if attempt or time.monotonic() - started >= 30:
                return response
            await asyncio.sleep(1)
        return response
    finally:
        slots.release()


async def _duanju_download_once(request, item_id, definition, playback_compat, playback_stream):
    """下载并解密剧集,直接返回可播放 MP4。

    流程: multi_video_model → spade_a 派生 AES key → 下载加密 MP4 → CENC AES-CTR 解密 → 返回明文 MP4。
    """
    started = time.perf_counter()
    try:
        model = _fetch_video_model(request, item_id)
        data = await while_connected(model, request.is_disconnected) if playback_compat else await model
        model_done = time.perf_counter()
        source = _pick_source(data['sources'], definition, prefer_h264=playback_compat, compatible_only=True)
        request.state.download_stage = '播放密钥解析'
        key_hex = derive_key_from_spade_a(source['spade_a'])
        request.state.download_stage = '视频 CDN 下载'
        download = _download_encrypted(source['urls'], getattr(request.app.state, 'video_client', None))
        encrypted = await while_connected(download, request.is_disconnected) if playback_compat else await download
        download_done = time.perf_counter()
    except (RuntimeError, ValueError) as exc:
        logger.warning('短剧下载失败: %s', exc)
        return error(str(exc), code=-8, status_code=502)

    try:
        request.state.download_stage = '视频解密'
        decrypted = await asyncio.to_thread(decrypt_mp4, encrypted, key_hex)
    except ValueError as exc:
        logger.error('短剧解密失败: %s', exc)
        return error(f'视频解密失败: {exc}', code=-9, status_code=502)

    logger.info('视频准备耗时: %s %s, 地址 %.3fs, CDN %.3fs, 解密 %.3fs', item_id, source['definition'], model_done-started, download_done-model_done, time.perf_counter()-download_done)
    if playback_compat:
        request.state.download_stage = '播放兼容转换'
        try:
            if playback_stream:
                stream, metadata = await prepare_streaming_video(decrypted, request.is_disconnected)
                return StreamingResponse(stream, media_type='video/mp4', headers={
                    'Cache-Control': 'no-store', 'X-Duanju-Playback': 'h264-aac-stream',
                    'X-Playback-Mime': metadata['mime'], 'X-Playback-Duration': metadata['duration'],
                    'X-Duanju-Definition': str(source['definition']),
                })
            decrypted = await prepare_compatible_video(decrypted, request.is_disconnected)
        except RuntimeError as exc:
            logger.warning('播放兼容处理失败: %s', exc)
            return error(str(exc), code=-10, status_code=502)

    logger.info('短剧解密完成: %s %s, %d→%d 字节', item_id, source['definition'], len(encrypted), len(decrypted))
    return Response(
        content=decrypted,
        media_type='video/mp4',
        headers={
            'Content-Disposition': f'attachment; filename="{item_id}_{source["definition"]}.mp4"',
            'Content-Length': str(len(decrypted)),
            'X-Duanju-Definition': str(source['definition']),
            'X-Duanju-Playback': 'h264-aac' if playback_compat else 'original',
            'Cache-Control': 'no-store' if playback_compat else 'public, max-age=86400',
        },
    )


@router.get('/duanju/categories')
async def duanju_categories(
    request: Request,
    content_type: str = Query(
        'drama', pattern='^(drama|manju)$', description='drama=真人剧,manju=漫剧',
    ),
):
    tab_type = MANJU_BOOKMALL_TAB if content_type == 'manju' else DRAMA_BOOKMALL_TAB
    cache_key = make_cache_key('duanju_category_groups', tab_type=tab_type)
    cached = _duanju_discovery_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        query = (
            f'tab_version=double_col&client_template=12'
            f'&bottom_tab_type_list=7%2C0%2C1%2C5%2C2%2C4'
            f'&tab_type={tab_type}&book_id=0&landing_bottom_tab_type=7'
            f'&bottom_tab_type=0&client_req_type=1&offset=0&device_level=3'
        )
        return f'{BOOKMALL_API}/reading/bookapi/bookmall/tab/v?{query}&{_bookmall_params(device_id)}'

    result = await request.app.state.client.call_with_device(
        build_url, aid=8662, max_device_retries=3,
    )
    if not result['ok']:
        logger.error('分组分类请求失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    groups = _category_groups(_bookmall_selector(result['upstream'], tab_type))
    data = {'content_type': content_type, 'groups': groups}
    _duanju_discovery_cache.set(cache_key, data)
    return success(data)


def _discovery_url(device_id, tab_type, filter_ids, selected_items) -> str:
    base = (
        f'tab_version=double_col&client_template=12'
        f'&bottom_tab_type_list=7%2C0%2C1%2C5%2C2%2C4'
        f'&tab_type={tab_type}&book_id=0&landing_bottom_tab_type=7&bottom_tab_type=0'
        f'&client_req_type=1&offset=0&device_level=3'
    )
    if filter_ids:
        base += f'&filter_ids={quote(filter_ids, safe=",")}'
    if selected_items:
        base += f'&selected_items={quote(selected_items, safe=",")}'
    return f'{BOOKMALL_API}/reading/bookapi/bookmall/tab/v?{base}&{_bookmall_params(device_id)}'

def _discovery_more_url(device_id, tab_type, cell_id, offset, session_id, plan_id, filter_ids, selected_items) -> str:
    gid_list = [sid for sid in filter_ids.split(',') if sid][:6]
    refresh_info = json.dumps({
        'first_screen_impression_gids': gid_list,
        'has_active_refresh': True,
        'latest_impression_gids': ','.join(gid_list),
        'refresh_type': 1,
    }, separators=(',', ':'))
    ug_task = json.dumps({'operation_type': 2}, separators=(',', ':'))
    query = (
        f'tab_version=double_col&client_template=12&change_type=0&cell_id={quote(cell_id, safe="")}'
        f'&offset={offset}&limit=0&tab_type={tab_type}&client_req_type=2'
        f'&unlimited_selector_change_type=1&page=0&page_entry_time=0&cold_start_session=0'
        f'&ecom_impression_start_time=0&last_dynamic_cover_offset=0&ecom_sort_by=0'
        f'&source_tab_type=0&search_tab_type=0&pad_column_detail=0&author_id=0'
        f'&book_comment_id=0&category_id=0&genre=0&genre_type=0&sub_genre=0&tag_id=0'
        f'&inner_category_id=0&rank_sub_info_id=0&item_id=0&total_chapter_num=0'
        f'&current_chapter_num=0&cell_sub_id=0&sub_tag_id=0&idol_tag_id=0'
        f'&ecom_category_id=0&video_tab_cold_start=0&version_tag=&device_level=3'
        f'&plan_id={quote(plan_id, safe="")}&session_id={quote(session_id, safe="")}'
        f'&screen_width_px=1271&last_impression_rec_tags=&selected_items={quote(selected_items, safe=",")}'
        f'&recent_impr_gid=&ClickedContent=&app_launch_times=0'
        f'&web_page_version_code=0&support_gender_list=false'
        f'&disable_digg_stat=false&ecom_refresh_type=0&pad_column_cover=0'
        f'&ug_task_params={quote(ug_task, safe="")}'
    )
    if gid_list:
        query += f'&first_screen_impression_gids={quote(",".join(gid_list), safe=",")}'
        query += f'&refresh_action_info={quote(refresh_info, safe="")}'
    if filter_ids:
        query += f'&filter_ids={quote(filter_ids, safe=",")}'
    return f'{BOOKMALL_API}/reading/bookapi/bookmall/cell/change/v?{query}&{_bookmall_params(device_id)}'

@router.get('/duanju/discovery')
async def duanju_discovery(
    request: Request,
    content_type: str = Query('drama', pattern='^(drama|manju)$', description='drama=真人剧,manju=漫剧'),
    filter_ids: str = Query('', description='可选,已展示 series_id 队列(换一换去重),由本接口响应自动累积'),
    selected_items: str = Query('', description='分类筛选,如 cate_20; 来自首屏 categories[].id'),
):
    """剧场栏发现页首屏(番茄 bookmall/tab/v,真人剧 tab_type=38 / 漫剧 tab_type=32)。

    响应包含翻页所需全部状态: cell_id / next_offset / session_id / plan_id / filter_ids,
    调用方原样传给 /duanju/discovery/more 即可获取下一页。
    """
    tab_type = MANJU_BOOKMALL_TAB if content_type == 'manju' else DRAMA_BOOKMALL_TAB
    cache_key = make_cache_key('duanju_discovery_v3', tab_type=tab_type, filter_ids=filter_ids, selected_items=selected_items)
    cached = _duanju_discovery_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        return _discovery_url(device_id, tab_type, filter_ids, selected_items)

    result = await request.app.state.client.call_with_device(build_url, aid=8662, max_device_retries=3)
    if not result['ok']:
        logger.error('发现页首屏失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    parsed = _parse_bookmall_tab(result['upstream'], tab_type)
    if not parsed['items']:
        return error('发现页首屏未返回推荐内容', code=-4, status_code=502)
    data = {
        'tab_type': tab_type,
        'items': parsed['items'],
        'cell_id': parsed['cell_id'],
        'next_offset': parsed['next_offset'],
        'has_more': bool(parsed['has_more'] and parsed['next_offset'] > 0
                         and all(parsed[key] for key in ('cell_id', 'session_id', 'plan_id'))),
        'session_id': parsed['session_id'],
        'plan_id': parsed['plan_id'],
        'filter_ids': _merge_filter_ids(filter_ids, parsed['items']),
        'tab_list': parsed['tab_list'],
        'categories': parsed.get('categories') or [],
        'rank_boards': _rank_boards(parsed['items']),
        'selected_items': selected_items,
        'raw': result['upstream'],
    }
    _duanju_discovery_cache.set(cache_key, data)
    return success(data)


@router.get('/duanju/discovery/more')
async def duanju_discovery_more(
    request: Request,
    content_type: str = Query('drama', pattern='^(drama|manju)$', description='drama=真人剧,manju=漫剧'),
    cell_id: str = Query(..., min_length=1, description='首屏返回的推荐 cell_id'),
    offset: int = Query(..., ge=1, description='上一页返回的 next_offset'),
    session_id: str = Query(..., min_length=1, description='上一页返回的 session_id(每页轮换)'),
    plan_id: str = Query(..., min_length=1, description='首屏返回的 plan_id(bookstore_id)'),
    filter_ids: str = Query('', description='上一页返回的 filter_ids(series_id 去重队列,防重复关键)'),
    selected_items: str = Query('', description='分类筛选,如 cate_20; 筛选后翻页需原样回传'),
):
    """剧场栏发现页翻页(bookmall/cell/change/v)。

    每次响应返回新的 session_id / next_offset / filter_ids,继续原样回传即可无限翻页。
    """
    tab_type = MANJU_BOOKMALL_TAB if content_type == 'manju' else DRAMA_BOOKMALL_TAB
    cache_key = make_cache_key(
        'duanju_discovery_more', tab_type=tab_type, cell_id=cell_id,
        offset=offset, session_id=session_id, filter_ids=filter_ids, selected_items=selected_items,
    )
    cached = _duanju_discovery_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        return _discovery_more_url(device_id, tab_type, cell_id, offset, session_id, plan_id, filter_ids, selected_items)

    result = await request.app.state.client.call_with_device(build_url, aid=8662, max_device_retries=3)
    if not result['ok']:
        logger.error('发现页翻页失败: %s', result['msg'])
        return error(result['msg'], code=-3, status_code=502)
    parsed = _parse_bookmall_change(result['upstream'])
    upstream_data = result['upstream'].get('data') or {}
    terminal_page = (isinstance(upstream_data, dict)
                     and upstream_data.get('has_more') in (False, 0)
                     and isinstance(upstream_data.get('cell_view'), dict))
    if not parsed['items'] and not terminal_page:
        return error('发现页翻页未返回内容', code=-4, status_code=502)
    new_filter = _merge_filter_ids(filter_ids, parsed['items'])
    data = {
        'tab_type': tab_type,
        'items': parsed['items'],
        'next_offset': parsed['next_offset'],
        'has_more': bool(parsed['has_more'] and parsed['next_offset'] > offset),
        'session_id': parsed['session_id'] or session_id,
        'cell_id': parsed['cell_id'] or cell_id,
        'plan_id': plan_id,
        'filter_ids': new_filter,
        'selected_items': selected_items,
        'rank_boards': _rank_boards(parsed['items']),
        'raw': result['upstream'],
    }
    _duanju_discovery_cache.set(cache_key, data)
    return success(data)


@router.get('/duanju/discovery/categories')
async def duanju_discovery_categories(request: Request):
    """旧版分类发现页(兼容保留): new_category/video/info/v。"""
    cache_key = 'duanju_discovery_categories'
    cached = _duanju_discovery_cache.get(cache_key)
    if cached is not None:
        return success(cached)

    def build_url(device_id: str) -> str:
        return f'{BASE_API}/reading/bookapi/new_category/video/info/v/?{_business_params_v70132(device_id)}'

    result = await request.app.state.client.call_with_device(build_url, max_device_retries=3)
    if not result['ok']:
        return error(result['msg'], code=-3, status_code=502)
    categories = (result['upstream'].get('data') or {}).get('category_list') or []
    data = {'items': categories, 'raw': result['upstream']}
    _duanju_discovery_cache.set(cache_key, data)
    return success(data)


@router.get('/duanju/rank')
async def duanju_rank(
    request: Request,
    board: str = Query('ranklist_hot_sc', description='抓包验证的 8 个榜单之一'),
    release_type: str = Query(
        'all', alias='type', pattern='^(all|playlet|comic_series_rank|ai_playlet)$',
        description='榜单类型: all=全部, playlet=真人剧, comic_series_rank=漫剧, ai_playlet=AI剧',
    ),
    cursor: str = Query('', description='后端生成的短游标'),
    limit: int = Query(20, ge=1, le=20),
):
    """抓包验证的 8 榜，支持按真人剧/漫剧/AI剧隔离分页。"""
    # 兼容直接调用路由函数的测试/内部调用：FastAPI 注入前默认值仍是 Query 对象。
    release_type = getattr(release_type, 'default', release_type)
    if board not in RANK_BOARDS:
        return error(f'不支持的榜单: {board}', code=-1, status_code=400)
    date = datetime.now(SHANGHAI).date().isoformat()
    cursor_type = f'rank:{release_type}:{board}'
    try:
        state = (
            _duanju_rank_cursors.get(cursor, cursor_type, date)
            if cursor
            else {'upstream': None, 'has_more': True, 'seen': [], 'buffer': []}
        )
        upstream_state = state.get('upstream')
        upstream_has_more = bool(state.get('has_more', True))
        seen_ids = [str(value) for value in state.get('seen') or [] if value]
        seen = set(seen_ids)
        available = [item for item in state.get('buffer') or [] if isinstance(item, dict)]

        while len(available) < limit and upstream_has_more:
            current_offset = int((upstream_state or {}).get('offset') or 0)
            preferred_device_id = str((upstream_state or {}).get('device_id') or '')
            selector_type = str(
                (upstream_state or {}).get('selector_type')
                or rank_selector(release_type, board)
            )

            parsed = await _fetch_rank_feed_page(
                request.app.state.client, selector_type, board, upstream_state,
                target_date=date, preferred_device_id=preferred_device_id,
            )
            preferred_device_id = parsed['device_id']
            raw_items = parsed['items']
            parsed['items'] = _filter_release_items(raw_items, release_type)
            if (
                release_type != 'all'
                and selector_type != 'all'
                and (not parsed['items'] or len(parsed['items']) != len(raw_items))
            ):
                # 部分榜单不响应类型筛选（例如热搜榜+漫剧），回退到混合榜后本地隔离。
                selector_type = 'all'
                current_offset = 0
                parsed = await _fetch_rank_feed_page(
                    request.app.state.client, selector_type, board, None,
                    target_date=date, preferred_device_id=preferred_device_id,
                )
                preferred_device_id = parsed['device_id']
                parsed['items'] = _filter_release_items(parsed['items'], release_type)
            next_offset = int(parsed.get('next_offset') or 0)
            upstream_has_more = bool(parsed.get('has_more'))
            if upstream_has_more and next_offset <= current_offset:
                raise RuntimeError('上游分页 offset 未前进')
            for item in parsed['items']:
                series_id = str(item.get('series_id') or '')
                if not series_id or series_id in seen:
                    continue
                seen.add(series_id)
                seen_ids.append(series_id)
                available.append({**item, 'release_type': item.get('release_type', '') if release_type == 'all' else release_type})
            upstream_state = {
                'offset': next_offset,
                'session_id': str(parsed.get('session_id') or ''),
                'rank_version': str(parsed.get('rank_version') or ''),
                'filter_ids': seen_ids[-200:],
                'device_id': preferred_device_id,
                'selector_type': selector_type,
            }

        items = available[:limit]
        buffer = available[limit:]
        has_more = bool(buffer or upstream_has_more)
        next_cursor = ''
        if has_more:
            next_cursor = _duanju_rank_cursors.put(
                cursor_type,
                date,
                {
                    'upstream': upstream_state,
                    'has_more': upstream_has_more,
                    'seen': seen_ids,
                    'buffer': buffer,
                },
            )
    except ValueError as exc:
        return error(str(exc), code=-2, status_code=400)
    except RuntimeError as exc:
        logger.warning('排行榜请求失败: %s', exc)
        return error(str(exc), code=-3, status_code=502)

    return success({
        'items': items,
        'next_cursor': next_cursor,
        'has_more': has_more,
        'board': board,
        'board_name': RANK_BOARDS[board],
        'source_note': ('上游暂无独立的该类型榜单，以下为综合榜中符合类型的剧目；数量以综合榜实际收录为准。' if release_type != 'all' and (upstream_state or {}).get('selector_type') == 'all' else ''),
        'release_type': release_type,
        'boards': [
            {'id': board_id, 'name': name}
            for board_id, name in RANK_BOARDS.items()
        ],
    })
