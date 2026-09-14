"""Bounded, replayable recommendation cursors. A device owns its whole page chain."""
import asyncio
import copy
import secrets
import time
from collections import OrderedDict


class CursorExpired(ValueError):
    pass


class RecommendationCollector:
    def __init__(self, ttl=1200, capacity=256, clock=time.monotonic):
        self.ttl = ttl
        self.capacity = capacity
        self.clock = clock
        self.entries = OrderedDict()
        self.lock = asyncio.Lock()

    async def page(self, tab, cursor, choose, fetch):
        # Automation requests one page at a time. Serialize replay and token issuance
        # so a timed-out/retried request cannot advance the upstream session twice.
        async with self.lock:
            now = self.clock()
            for key in list(self.entries):
                if now - self.entries[key]['at'] >= self.ttl:
                    del self.entries[key]
            previous = None
            if cursor:
                previous = self.entries.get(cursor)
                if previous is None or previous['tab'] != tab:
                    raise CursorExpired('推荐游标已失效，重新取首屏')
                if previous.get('response') is not None:
                    return copy.deepcopy(previous['response'])
                device, state = previous['device'], copy.deepcopy(previous['state'])
            else:
                device, state = await choose(), {}
            parsed = await fetch(device, tab, state)
            if not isinstance(parsed.get('items'), list):
                raise RuntimeError('推荐内容响应不完整')
            seen = list(state.get('seen', []))
            seen_set = set(seen)
            fresh = []
            for item in parsed['items']:
                sid = str(item.get('series_id') or item.get('book_id') or '')
                if sid and sid not in seen_set:
                    fresh.append(item)
                    seen.append(sid)
                    seen_set.add(sid)
            next_state = {**state, **{k: parsed[k] for k in
                ('cell_id', 'plan_id', 'session_id', 'next_offset') if parsed.get(k)}}
            offset = int(parsed.get('next_offset') or 0)
            reason = ''
            more = bool(parsed.get('has_more'))
            if more and not fresh:
                more, reason = False, '推荐页为空或重复，切换下一设备'
            elif more and (offset <= int(state.get('next_offset', 0)) or
                           not all(next_state.get(k) for k in ('cell_id', 'plan_id', 'session_id'))):
                more, reason = False, '推荐分页未前进或缺少会话，切换下一设备'
            elif not more:
                reason = '当前设备推荐已结束'
            next_cursor = ''
            if more:
                # Page bounds in automation cap a chain at 30 pages. A separate
                # hard bound protects callers using this endpoint directly.
                next_state['seen'] = seen[-2000:]
                next_state['filter_ids'] = ','.join(seen[-200:])
                next_cursor = secrets.token_urlsafe(24)
                self.entries[next_cursor] = dict(at=self.clock(), tab=tab, device=device,
                                                  state=next_state, response=None)
            response = dict(items=fresh, has_more=more, next_cursor=next_cursor,
                            reason=reason, tab_type=tab)
            if previous is not None:
                previous['response'] = copy.deepcopy(response)
            while len(self.entries) > self.capacity:
                self.entries.popitem(last=False)
            return response
