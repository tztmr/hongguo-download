import copy
import unittest

from endpoints.duanju import _duanju_discovery_cache, duanju_discovery, duanju_discovery_more
from tests.test_duanju_extended import RecordingClient, request_for, response_json


def cell():
    return {'cell_id': 'recommend-cell', 'next_offset': 6,
            'cell_data': [{'video_data': [{'series_id': 'series-1', 'title': '示例剧'}]}]}


class HomeDiscoveryTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        _duanju_discovery_cache.clear()

    async def first(self, tab):
        async def handler(**_kwargs):
            return {'ok': True, 'upstream': {'data': {'tab_item': [tab]}}}
        return response_json(await duanju_discovery(request_for(RecordingClient(handler)), 'drama', '', ''))

    async def more(self, data):
        async def handler(**_kwargs):
            return {'ok': True, 'upstream': {'data': data}}
        response = await duanju_discovery_more(request_for(RecordingClient(handler)), 'drama',
                                               'recommend-cell', 6, 'session', 'plan', 'series-1', 'cate_20')
        return response.status_code, response_json(response)

    async def test_first_page_honors_terminal_state_and_requires_complete_pagination(self):
        tab = {'tab_type': '38', 'session_id': 'session', 'bookstore_id': 'plan', 'has_more': True, 'cell_data': [cell()]}
        self.assertTrue((await self.first(tab))['data']['has_more'])
        for key, value in [('has_more', False), ('session_id', ''), ('bookstore_id', '')]:
            with self.subTest(key=key):
                _duanju_discovery_cache.clear()
                changed = copy.deepcopy(tab)
                changed[key] = value
                data = (await self.first(changed))['data']
                self.assertEqual(len(data['items']), 1)
                self.assertFalse(data['has_more'])

    async def test_empty_terminal_page_is_success_but_missing_response_is_retryable(self):
        status, result = await self.more({'has_more': False, 'cell_view': {}})
        self.assertEqual(status, 200)
        self.assertEqual(result['data']['items'], [])
        self.assertFalse(result['data']['has_more'])
        for body in ({}, {'cell_view': {}}, {'has_more': True, 'cell_view': {}}):
            _duanju_discovery_cache.clear()
            status, result = await self.more(body)
            self.assertEqual(status, 502)
            self.assertEqual(result['code'], -4)

    async def test_repeated_offset_stops_and_new_cell_and_category_are_carried_forward(self):
        changed = cell()
        changed['cell_id'] = 'next-cell'
        status, result = await self.more({'has_more': True, 'cell_view': changed, 'next_offset': 6, 'session_id': 'next-session'})
        self.assertEqual(status, 200)
        data = result['data']
        self.assertFalse(data['has_more'])
        self.assertEqual(data['selected_items'], 'cate_20')
        self.assertEqual(data['cell_id'], 'next-cell')
        self.assertEqual(data['session_id'], 'next-session')
