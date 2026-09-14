import unittest
from types import SimpleNamespace

from core.recommendation_collector import RecommendationCollector, CursorExpired


class RecommendationCollectorTest(unittest.IsolatedAsyncioTestCase):
    async def test_pages_pin_device_rotate_session_and_replay_without_fetch(self):
        calls = []
        devices = iter(['one', 'two'])

        async def choose():
            return next(devices)

        async def fetch(device, tab, state):
            calls.append((device, dict(state)))
            n = len(calls)
            return dict(items=[{'series_id': str(7680000000000000000 + n)}],
                        cell_id='cell', plan_id='plan', session_id=f's{n}',
                        next_offset=n, has_more=True)

        collector = RecommendationCollector()
        first = await collector.page('32', '', choose, fetch)
        second = await collector.page('32', first['next_cursor'], choose, fetch)
        replay = await collector.page('32', first['next_cursor'], choose, fetch)
        self.assertEqual(second, replay)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1][0], 'one')
        self.assertEqual(calls[1][1]['session_id'], 's1')
        self.assertEqual(calls[1][1]['filter_ids'], '7680000000000000001')
        await collector.page('32', '', choose, fetch)
        self.assertEqual(calls[-1][0], 'two')
        self.assertEqual(calls[-1][1], {})

    async def test_repeat_page_and_nonadvancing_offset_end_device_round(self):
        async def choose(): return 'device'
        async def fetch(device, tab, state):
            return dict(items=[{'series_id': '1'}], cell_id='c', plan_id='p',
                        session_id='s', next_offset=1, has_more=True)
        collector = RecommendationCollector()
        first = await collector.page('32', '', choose, fetch)
        second = await collector.page('32', first['next_cursor'], choose, fetch)
        self.assertFalse(second['has_more'])
        self.assertEqual(second['items'], [])
        self.assertIn('重复', second['reason'])

    async def test_terminal_empty_is_end_but_malformed_page_is_failure(self):
        async def choose(): return 'device'
        async def first(device, tab, state):
            return dict(items=[{'series_id': '1'}], cell_id='c', plan_id='p',
                        session_id='s', next_offset=1, has_more=True)
        async def terminal(device, tab, state):
            return dict(items=[], has_more=False, next_offset=0)
        async def invalid(device, tab, state):
            raise RuntimeError('上游响应不完整')
        collector = RecommendationCollector()
        p = await collector.page('32', '', choose, first)
        with self.assertRaises(RuntimeError):
            await collector.page('32', p['next_cursor'], choose, invalid)
        p = await collector.page('32', p['next_cursor'], choose, terminal)
        self.assertFalse(p['has_more'])

    async def test_expired_or_other_tab_cursor_never_moves_to_another_device(self):
        clock = [10.0]
        collector = RecommendationCollector(ttl=5, clock=lambda: clock[0])
        async def choose(): return 'device'
        async def fetch(device, tab, state):
            return dict(items=[{'series_id': '1'}], cell_id='c', plan_id='p',
                        session_id='s', next_offset=1, has_more=True)
        page = await collector.page('32', '', choose, fetch)
        with self.assertRaises(CursorExpired):
            await collector.page('8', page['next_cursor'], choose, fetch)
        clock[0] = 16
        with self.assertRaises(CursorExpired):
            await collector.page('32', page['next_cursor'], choose, fetch)

    async def test_missing_first_page_session_keeps_items_but_does_not_fake_more(self):
        async def choose(): return 'device'
        async def fetch(device, tab, state):
            return dict(items=[{'series_id': '1'}], cell_id='c', next_offset=1, has_more=True)
        result = await RecommendationCollector().page('32', '', choose, fetch)
        self.assertEqual(len(result['items']), 1)
        self.assertFalse(result['has_more'])
        self.assertEqual(result['next_cursor'], '')
