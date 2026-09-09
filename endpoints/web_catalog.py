"""Website category filters, independent of signed bookmall cursor sessions."""

import logging

import httpx
from fastapi import APIRouter, Query

from core.cache import TTLCache, make_cache_key
from core.response import error, success
from core.web_catalog import (
    WEB_ORIGIN,
    CatalogFormatError,
    build_category_url,
    parse_category_page,
    parse_selector_html,
)

logger = logging.getLogger('fanqie.web_catalog')
router = APIRouter()
_catalog_cache = TTLCache(default_ttl=60, max_size=256)


async def _fetch_public(url: str):
    async with httpx.AsyncClient(timeout=15.0, headers={'User-Agent': 'Mozilla/5.0', 'Accept': 'application/json,text/html'}) as client:
        response = await client.get(url)
        response.raise_for_status()
        if url == f'{WEB_ORIGIN}/category':
            return response.text
        try:
            return response.json()
        except ValueError as exc:
            raise CatalogFormatError('官网分类返回了无效数据') from exc


async def _load_groups():
    cached = _catalog_cache.get('groups')
    if cached is not None:
        return cached
    groups = parse_selector_html(await _fetch_public(f'{WEB_ORIGIN}/category'))
    _catalog_cache.set('groups', groups, ttl=300)
    return groups


def _unsupported_comics():
    return error('官网漫画分类不等同于漫剧，请使用漫剧推荐分类', status_code=400)


@router.get('/duanju/web-categories')
async def web_categories(content_type: str = Query('drama', pattern='^(drama|manju)$')):
    if content_type != 'drama':
        return _unsupported_comics()
    try:
        return success({'groups': await _load_groups(), 'source': 'hongguo_web', 'source_url': f'{WEB_ORIGIN}/category'})
    except (httpx.HTTPError, CatalogFormatError) as exc:
        logger.warning('Public category dictionary failed: %s', type(exc).__name__)
        return error('官网分类暂不可用，请稍后重试', code=502, status_code=502)


@router.get('/duanju/web-category')
async def web_category(
    content_type: str = Query('drama', pattern='^(drama|manju)$'),
    background: str = Query('', pattern=r'^(cate_\d+)?$'),
    topic: str = Query('', pattern=r'^(cate_\d+)?$'),
    setting: str = Query('', pattern=r'^(cate_\d+)?$'),
    gender: str = Query('2', pattern='^[012]$'),
    time: str = Query('0', pattern='^[0-4]$'),
    sort_type: str = Query('0', pattern='^[012]$'),
    page: int = Query(1, ge=1, le=1000),
):
    if content_type != 'drama':
        return _unsupported_comics()
    filters = dict(background=background, topic=topic, setting=setting, gender=gender, time=time, sort_type=sort_type, page=page)
    cache_key = make_cache_key('page', **filters)
    cached = _catalog_cache.get(cache_key)
    if cached is not None:
        return success(cached)
    try:
        if any((background, topic, setting)):
            for group in await _load_groups():
                selected = filters.get(group['id'])
                if selected and str(selected) not in {option['id'] for option in group['items']}:
                    return error(f"无效的{group['name']}筛选，请刷新分类后重试", status_code=400)
        payload = await _fetch_public(build_category_url(**filters))
        data = parse_category_page(payload, page=page)
    except (httpx.HTTPError, CatalogFormatError) as exc:
        logger.warning('Public category page failed: %s', type(exc).__name__)
        return error('官网分类加载失败，请重试；当前筛选条件已保留', code=502, status_code=502)
    _catalog_cache.set(cache_key, data)
    return success(data)
