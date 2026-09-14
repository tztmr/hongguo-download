"""Session-bound recommendation pages for background discovery."""
import asyncio

from fastapi import APIRouter, Query, Request

from core.device_pool import device_pool
from core.recommendation_collector import CursorExpired, RecommendationCollector
from core.response import error, success
from endpoints.duanju import (
    BOOKMALL_API, FEED_API, _discovery_url, _discovery_more_url,
    _parse_bookmall_tab, _parse_bookmall_change,
)

router = APIRouter()
collector = RecommendationCollector()


async def fetch_recommendation(client, device_id, tab, state):
    device = device_pool.get_device(device_id)
    if device is None:
        raise CursorExpired('当前推荐设备已失效，重新选择设备取首屏')
    if state:
        url = _discovery_more_url(device_id, tab, state['cell_id'], state['next_offset'],
                                  state['session_id'], state['plan_id'], state['filter_ids'], '')
    else:
        url = _discovery_url(device_id, tab, '', '')
    if tab == '8':
        # The documented find-drama tab returns its live cell/plan IDs. Never
        # reuse the sample tab-16 cell constants as permanent pagination state.
        url = url.replace(BOOKMALL_API, FEED_API).replace('/v?', '/v:version/?')
    url += '&need_personal_recommend=1'
    result = await asyncio.wait_for(client._do_call(
        url, method='GET', data='', aid=8662, _retry=0, device=device), timeout=25)
    if not result.get('ok'):
        device_pool.report_failure(device_id)
        raise RuntimeError('推荐请求失败，稍后换设备自动重试')
    upstream = result.get('upstream') or {}
    data = upstream.get('data') or {}
    if state:
        # A valid terminal empty page is different from a missing/invalid body.
        if not isinstance(data, dict) or 'has_more' not in data or 'cell_view' not in data:
            raise RuntimeError('推荐分页响应不完整，稍后自动恢复')
        parsed = _parse_bookmall_change(upstream)
        if parsed['cell_id'] and parsed['cell_id'] != state['cell_id']:
            raise CursorExpired('推荐模块已变化，重新取首屏')
    else:
        parsed = _parse_bookmall_tab(upstream, tab)
        if not parsed['items']:
            raise RuntimeError('当前设备未返回推荐内容，稍后换设备自动重试')
    device_pool.report_success(device_id)
    return parsed


@router.get('/duanju/collection/recommendation')
async def recommendation_page(
    request: Request,
    tab: str = Query('32', pattern='^(32|38|8)$'),
    cursor: str = Query('', max_length=128),
):
    client = request.app.state.client

    async def choose():
        # Existing healthy devices rotate by least-recent-use. Collection never
        # registers a new identity just to rotate recommendations.
        device = device_pool.get_best_device()
        if device is None:
            raise RuntimeError('暂无可用推荐设备，等待设备池恢复')
        return device.device_id

    async def fetch(device, selected, state):
        return await fetch_recommendation(client, device, selected, state)

    try:
        return success(await collector.page(tab, cursor, choose, fetch))
    except CursorExpired as exc:
        return success(dict(items=[], has_more=False, next_cursor='', reset_cursor=True, reason=str(exc)))
    except (RuntimeError, asyncio.TimeoutError, ValueError, TypeError, KeyError):
        return error('推荐采集暂不可用，保留其他来源扫描并自动重试', code=-3, status_code=502)
