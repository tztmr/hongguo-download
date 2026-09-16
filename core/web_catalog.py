"""Public hongguoduanju.com category adapter (verified 2026-09-16).

The site exposes its category dictionary in JSON inside /category HTML, and
serves result pages from /api/category/page. No external JavaScript is executed.
Video categories use tab=1 and separate RPC content types. tab=2 is manga.
"""

import json
import re
from html.parser import HTMLParser
from typing import Any
from urllib.parse import urlencode

WEB_ORIGIN = 'https://hongguoduanju.com'
WEB_CATEGORY_URL = f'{WEB_ORIGIN}/category/real-drama'
CATALOG_TYPES = {
    'drama': ('real-drama', 1, 1, 'playlet'),
    'manju': ('comic-drama', 3, 1004, 'comic_series_rank'),
    'ai': ('ai-drama', 4, 1004, 'ai_playlet'),
}


def category_source_url(content_type='drama') -> str:
    return f'{WEB_ORIGIN}/category/{CATALOG_TYPES[content_type][0]}'


GROUPS = (
    ('background', '背景', ''),
    ('topic', '主题', ''),
    ('setting', '设定', ''),
    ('gender', '受众', '2'),
    ('time', '时间', '0'),
    ('sort_type', '推荐', '0'),
)


class CatalogFormatError(ValueError):
    """An upstream failure or protocol change, not an empty result set."""


class _Scripts(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.scripts: list[str] = []
        self._parts: list[str] | None = None

    def handle_starttag(self, tag, attrs):
        if tag == 'script':
            self._parts = []

    def handle_data(self, data):
        if self._parts is not None:
            self._parts.append(data)

    def handle_endtag(self, tag):
        if tag == 'script' and self._parts is not None:
            self.scripts.append(''.join(self._parts))
            self._parts = None


def _text(value: Any) -> str:
    return re.sub(r'[\x00-\x1f\x7f]', '', str(value if value is not None else '')).strip()


def parse_selector_html(html: str, *, content_type='drama') -> list[dict]:
    parser = _Scripts()
    parser.feed(html)
    page = None
    for script in parser.scripts:
        assignment = re.search(r'(?:^|;)\s*(?:window\.)?_ROUTER_DATA\s*=\s*', script)
        if not assignment:
            continue
        try:
            data, _ = json.JSONDecoder().raw_decode(script[assignment.end():])
            loaders = data.get('loaderData', {})
            candidate = loaders.get('category_$') or loaders.get('category_page')
        except (ValueError, AttributeError):
            continue
        if isinstance(candidate, dict):
            page = candidate
            break
    if not page or page.get('isSuccess') is not True:
        raise CatalogFormatError('官网分类字典返回格式已变化')
    rows = page.get('selectorList')
    if not isinstance(rows, list):
        raise CatalogFormatError('官网分类字典缺少筛选维度')
    route = page.get('categoryRoute')
    if isinstance(route, dict):
        if route.get('contentType') != CATALOG_TYPES[content_type][0]:
            raise CatalogFormatError('官网返回了不匹配的剧目分类')
        # The current site exposes one topic dimension with route slugs and RPC IDs.
        # These are not the old cate_N values used by background/topic/setting.
        items = [{'id': '', 'name': '全部'}]
        seen = {''}
        for row in rows:
            if not isinstance(row, dict) or str(row.get('row_id')) != '1':
                continue
            options = row.get('items')
            if not isinstance(options, list):
                continue
            for option in options:
                if not isinstance(option, dict):
                    continue
                slug, label = _text(option.get('selector_item_id')), _text(option.get('show_name'))
                ids = option.get('category_json_ids')
                if (re.fullmatch(r'[a-z][a-z0-9-]*', slug) and label and slug not in seen
                        and isinstance(ids, list) and ids
                        and all(isinstance(value, str) and value.isdigit() for value in ids)):
                    items.append({'id': slug, 'name': label, 'category_json_ids': list(dict.fromkeys(ids))})
                    seen.add(slug)
        if len(items) == 1:
            raise CatalogFormatError('官网分类字典的分类选项为空')
        return [{'id': 'topic', 'name': '分类', 'items': items}]
    if content_type != 'drama':
        raise CatalogFormatError('官网未返回对应的视频分类字典')
    result = []
    for row_id, (group_id, name, default) in enumerate(GROUPS, start=1):
        row = next((row for row in rows if isinstance(row, dict) and str(row.get('row_id')) == str(row_id)), None)
        if not row or not isinstance(row.get('items'), list):
            raise CatalogFormatError(f'官网分类字典缺少{name}维度')
        items = [{'id': default, 'name': '全部'}]
        seen = {default}
        for option in row['items']:
            if not isinstance(option, dict):
                continue
            item_id, label = _text(option.get('selector_item_id')), _text(option.get('show_name'))
            valid = re.fullmatch(r'cate_\d+', item_id) if row_id <= 3 else item_id.isdigit()
            if valid and label and item_id not in seen:
                items.append({'id': item_id, 'name': label})
                seen.add(item_id)
        if len(items) == 1:
            raise CatalogFormatError(f'官网分类字典的{name}选项为空')
        result.append({'id': group_id, 'name': name, 'items': items})
    return result


def build_category_url(*, content_type='drama', background='', topic='', setting='', gender='2', time='0', sort_type='0', page=1, category_json_ids=None) -> str:
    # min_first_visible_time is the site's 0..4 interval enum, not a timestamp.
    params = {
        'tab': '1', 'content_type': str(CATALOG_TYPES[content_type][1]), 'min_first_visible_time': time, 'gender': gender,
        'sort_type': sort_type, 'page_num': page,
    }
    categories = [value for value in (topic, setting, background) if value]
    if category_json_ids:
        params['category_json_ids'] = ','.join(category_json_ids)
    elif categories:
        params['categories_v2'] = ','.join(categories)
    return f'{WEB_ORIGIN}/api/category/page?{urlencode(params)}'


def _integer(value: Any, label: str) -> int:
    if isinstance(value, bool):
        raise CatalogFormatError(f'官网分类{label}格式无效')
    try:
        number = int(value)
    except (ValueError, TypeError, OverflowError) as exc:
        raise CatalogFormatError(f'官网分类{label}格式无效') from exc
    if number < 0 or (isinstance(value, float) and value != number):
        raise CatalogFormatError(f'官网分类{label}格式无效')
    return number


def parse_category_page(payload: Any, *, page: int, content_type='drama') -> dict:
    if not isinstance(payload, dict) or payload.get('isSuccess') is not True:
        raise CatalogFormatError('官网分类请求未成功')
    rows = payload.get('recommendList')
    if not isinstance(rows, list):
        raise CatalogFormatError('官网分类返回格式已变化')
    page_number = _integer(payload.get('pageNum'), '页码')
    page_size = _integer(payload.get('pageSize'), '分页大小')
    total = _integer(payload.get('total'), '总数')
    if page_number != page or not 1 <= page_size <= 1000:
        raise CatalogFormatError('官网分类返回了不匹配的分页信息')
    items, seen = [], set()
    for row in rows:
        if not isinstance(row, dict):
            continue
        series_id = _text(row.get('series_id'))
        if not series_id.isdigit() or series_id in seen:
            continue
        seen.add(series_id)
        vids = row.get('vid_list') if isinstance(row.get('vid_list'), list) else []
        first_vid = next((_text(vid) for vid in vids if _text(vid).isdigit()), '')
        tags = list(dict.fromkeys(_text(tag) for tag in row.get('tags', []) if isinstance(tag, str) and _text(tag))) if isinstance(row.get('tags'), list) else []
        episode_info = row.get('series_episode_info') if isinstance(row.get('series_episode_info'), dict) else {}
        items.append({
            'series_id': series_id, 'book_id': series_id,
            'title': _text(row.get('series_name')) or '未命名短剧',
            'cover': _text(row.get('series_cover')), 'first_vid': first_vid,
            'episode_count': _integer(row.get('episode_cnt') or episode_info.get('episode_cnt') or 0, '集数'),
            'content_type': CATALOG_TYPES[content_type][2], 'duration': 0, 'abstract': _text(row.get('series_intro')),
            'score': '', 'category_tags': tags, 'category': ' · '.join(tags),
            'release_type': CATALOG_TYPES[content_type][3], 'rank_tags': [],
            'online_time': None, 'play_count': None, 'hot_count': None,
            'collect_count': None, 'like_count': None, 'comment_count': None,
        })
    if rows and not items:
        raise CatalogFormatError('官网分类未返回可识别的短剧信息')
    if not rows and page_number * page_size < total:
        raise CatalogFormatError('官网分类返回空页但仍有后续结果，请重试')
    has_more = bool(items) and page_number * page_size < total
    return {
        'items': items, 'page': page_number, 'page_size': page_size,
        'next_page': page_number + 1 if has_more else None,
        'has_more': has_more, 'total': total,
        'source': 'hongguo_web', 'source_url': category_source_url(content_type),
    }
