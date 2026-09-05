"""头条 PSeries 短剧接口:公开 GET 请求,不使用项目签名或设备池。"""
from __future__ import annotations

import logging
from typing import Any

import httpx
from fastapi import APIRouter, Query, Request

from core.cache import TTLCache, make_cache_key
from core.response import error, success

logger = logging.getLogger('fanqie.pseries')
router = APIRouter()

PSERIES_URL = 'https://api5-normal-hl.toutiaoapi.com/ugc/video/v1/pseries'
DEFAULT_AID = 35
DEFAULT_COUNT = 200
_cache = TTLCache(default_ttl=300, max_size=256)


def _first_url(image: Any) -> str:
    if isinstance(image, dict):
        if isinstance(image.get('url'), str):
            return image['url']
        values = image.get('urlList')
        if isinstance(values, list):
            return next((url for url in values if isinstance(url, str)), '')
    if isinstance(image, list):
        return _first_url(image[0]) if image else ''
    return ''


def _normalize_episode(cell: dict[str, Any], index: int) -> dict[str, Any]:
    article = cell.get('articleBase') or {}
    video = cell.get('videoInfo') or {}
    series = cell.get('pSeriesInfo') or {}
    play_addr = video.get('playAddr') or {}
    urls = play_addr.get('playUrlList') or []
    if not isinstance(urls, list):
        urls = []
    return {
        'index': index,
        'episode_id': str(series.get('referGID') or article.get('groupID') or article.get('gidStr') or ''),
        'video_id': str(video.get('videoID') or ''),
        'title': article.get('title') or series.get('title') or '',
        'description': article.get('abstractText') or '',
        'duration': video.get('videoDuration') or 0,
        'width': video.get('width') or 0,
        'height': video.get('height') or 0,
        'definition': play_addr.get('definition') or '',
        'codec_type': play_addr.get('codecType') or '',
        'size': play_addr.get('size') or 0,
        'urls': [url for url in urls if isinstance(url, str) and url],
        'cover': _first_url((cell.get('imageList') or {}).get('firstFrameImageList')),
    }


def _normalize(payload: dict[str, Any], pseries_id: str) -> dict[str, Any]:
    cells = payload.get('item_cell_list') or []
    episodes = [_normalize_episode(cell, index) for index, cell in enumerate(cells, start=1) if isinstance(cell, dict)]
    first = cells[0] if cells and isinstance(cells[0], dict) else {}
    series = first.get('pSeriesInfo') or {}
    tags = series.get('tags') or {}
    return {
        'pseries_id': str(series.get('pSeriesID') or pseries_id),
        'title': series.get('title') or (first.get('articleBase') or {}).get('title') or '',
        'total': series.get('total') or len(episodes),
        'cover': _first_url(series.get('largeImageList')),
        'category': tags.get('multi_category') or tags.get('playlet_type') or '',
        'hot_value': tags.get('hot_value') or '',
        'update_status': tags.get('update_status') or '',
        'first_episode_id': tags.get('first_chapter_id') or (episodes[0]['episode_id'] if episodes else ''),
        'episodes': episodes,
        'has_more': bool(payload.get('has_more')),
        'next_cursor': payload.get('next') or payload.get('next_behot_time') or '',
    }


async def _fetch(request: Request, pseries_id: str, min_behot_time: str | None, max_behot_time: str | None, count: int) -> dict[str, Any]:
    min_time = min_behot_time or pseries_id
    max_time = max_behot_time or pseries_id
    cache_key = make_cache_key('pseries', pseries_id=pseries_id, min=min_time, max=max_time, count=count)
    cached = _cache.get(cache_key)
    if cached is not None:
        return cached
    params = {
        'pseries_id': pseries_id,
        'min_behot_time': min_time,
        'max_behot_time': max_time,
        'pseries_type': 8,
        'aid': DEFAULT_AID,
        'count': count,
    }
    try:
        response = await request.app.state.client._client.get(PSERIES_URL, params=params)
        response.raise_for_status()
        payload = response.json()
    except (httpx.HTTPError, ValueError) as exc:
        raise RuntimeError(f'PSeries 上游请求失败: {exc}') from exc
    if payload.get('status') not in (0, '0', None):
        raise RuntimeError(payload.get('message') or f"PSeries 上游状态异常: {payload.get('status')}")
    data = _normalize(payload, pseries_id)
    _cache.set(cache_key, data)
    return data


@router.get('/pseries/detail')
async def pseries_detail(
    request: Request,
    pseries_id: str = Query(..., min_length=1, description='PSeries 短剧 ID'),
    min_behot_time: str | None = Query(None, description='上游最小游标,缺省时使用 pseries_id'),
    max_behot_time: str | None = Query(None, description='上游最大游标,缺省时使用 pseries_id'),
    count: int = Query(DEFAULT_COUNT, ge=1, le=200, description='单次返回剧集数'),
):
    """无签名 PSeries 详情和剧集列表。"""
    try:
        return success(await _fetch(request, pseries_id, min_behot_time, max_behot_time, count))
    except RuntimeError as exc:
        logger.warning('%s', exc)
        return error(str(exc), code=-3, status_code=502)


@router.get('/pseries/catalog')
async def pseries_catalog(
    request: Request,
    pseries_id: str = Query(..., min_length=1, description='PSeries 短剧 ID'),
    min_behot_time: str | None = Query(None),
    max_behot_time: str | None = Query(None),
    count: int = Query(DEFAULT_COUNT, ge=1, le=200),
):
    """无签名 PSeries 目录,仅返回统一剧集字段。"""
    try:
        data = await _fetch(request, pseries_id, min_behot_time, max_behot_time, count)
        return success({
            'pseries_id': data['pseries_id'],
            'title': data['title'],
            'total': data['total'],
            'episodes': data['episodes'],
            'has_more': data['has_more'],
            'next_cursor': data['next_cursor'],
        })
    except RuntimeError as exc:
        logger.warning('%s', exc)
        return error(str(exc), code=-3, status_code=502)


@router.get('/pseries/content')
async def pseries_content(
    request: Request,
    pseries_id: str = Query(..., min_length=1, description='PSeries 短剧 ID'),
    episode_index: int = Query(..., ge=1, description='剧集序号,从 1 开始'),
    min_behot_time: str | None = Query(None),
    max_behot_time: str | None = Query(None),
    count: int = Query(DEFAULT_COUNT, ge=1, le=200),
):
    """按序号获取无加密 MP4 播放地址。"""
    try:
        data = await _fetch(request, pseries_id, min_behot_time, max_behot_time, count)
        if episode_index > len(data['episodes']):
            return error(f'剧集序号超出本页范围: {episode_index}/{len(data["episodes"])}', code=-4)
        episode = data['episodes'][episode_index - 1]
        if not episode['urls']:
            return error('该剧集没有可用播放地址', code=-4, status_code=502)
        return success(episode)
    except RuntimeError as exc:
        logger.warning('%s', exc)
        return error(str(exc), code=-3, status_code=502)
