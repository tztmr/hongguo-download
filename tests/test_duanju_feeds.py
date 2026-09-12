import unittest
from datetime import datetime
from urllib.parse import parse_qs, urlparse
from zoneinfo import ZoneInfo

from core.duanju_feeds import (
    RANK_BOARDS,
    TODAY_RELEASE_TYPES,
    build_rank_url,
    build_subscribe_url,
    parse_batch_metrics,
    parse_rank_page,
    parse_subscribe_page,
)


SHANGHAI = ZoneInfo("Asia/Shanghai")


class CapturedFeedProtocolTests(unittest.TestCase):
    def test_subscribe_url_uses_shanghai_target_date_and_captured_profile(self):
        url = build_subscribe_url(
            "dynamic-device", "20260903", offset=30, session_id="next-session"
        )
        parsed = urlparse(url)
        query = parse_qs(parsed.query, keep_blank_values=True)

        self.assertEqual(parsed.netloc, "api5-normal-sinfonlineb.fqnovel.com")
        self.assertEqual(parsed.path, "/reading/user/subscribe/list/v:version/")
        self.assertEqual(query["target_date"], ["20260903"])
        self.assertEqual(query["offset"], ["30"])
        self.assertEqual(query["session_id"], ["next-session"])
        self.assertEqual(query["tab_type"], ["5"])
        self.assertEqual(query["active_panel"], ["5"])
        self.assertEqual(query["filter_type"], ["gender"])
        self.assertEqual(query["gender_type"], ["2"])
        self.assertEqual(query["order_experiment"], ["descend"])
        self.assertEqual(query["aid"], ["8662"])
        self.assertEqual(query["app_name"], ["novelread"])
        self.assertEqual(query["version_name"], ["7.3.2.32"])
        self.assertEqual(query["device_id"], ["dynamic-device"])
        self.assertNotIn("cookie", query)
        self.assertNotIn("token", query)

    def test_subscribe_parser_keeps_only_online_rows_for_the_target_date(self):
        today = int(datetime(2026, 9, 3, 9, tzinfo=SHANGHAI).timestamp())
        yesterday = int(datetime(2026, 9, 2, 23, tzinfo=SHANGHAI).timestamp())

        def row(series_id, timestamp, *, online=True):
            return {
                "is_online": online,
                "schedule_publish_time": timestamp,
                "sub_title_list": ["青春校园", "古装仙侠"],
                "subscribe_data": {
                    "series_id": series_id,
                    "title": f"剧{series_id}",
                    "content_type": 1,
                    "episode_cnt": 20,
                    "play_cnt": 9,
                    "video_detail": {
                        "series_id": series_id,
                        "series_play_cnt": 0,
                        "followed_cnt": 7,
                    },
                },
            }

        page = parse_subscribe_page(
            {
                "data": {
                    "subscribe_items": [
                        row("today", today),
                        row("old", yesterday),
                        row("offline", today, online=False),
                    ],
                    "next_offset": 15,
                    "session_id": "rotated",
                    "has_more": True,
                }
            },
            "20260903",
        )

        self.assertEqual([item["series_id"] for item in page["items"]], ["today"])
        item = page["items"][0]
        self.assertEqual(item["release_type"], "playlet")
        self.assertEqual(item["online_time"], today)
        self.assertEqual(item["play_count"], 0)
        self.assertEqual(item["collect_count"], 7)
        self.assertIsNone(item["hot_count"])
        self.assertIsNone(item["like_count"])
        self.assertEqual(item["category_tags"], ["青春校园", "古装仙侠"])
        self.assertEqual(page["next_offset"], 15)
        self.assertEqual(page["session_id"], "rotated")
        self.assertTrue(page["has_more"])

    def test_rank_url_distinguishes_first_page_and_following_page_state(self):
        first = parse_qs(
            urlparse(
                build_rank_url(
                    "dynamic-device", "all", "ranklist_must_watch", state=None
                )
            ).query,
            keep_blank_values=True,
        )
        later = parse_qs(
            urlparse(
                build_rank_url(
                    "dynamic-device",
                    "all",
                    "ranklist_must_watch",
                    state={
                        "offset": 15,
                        "session_id": "rotated",
                        "rank_version": "version-2",
                        "filter_ids": ["a", "b"],
                    },
                )
            ).query,
            keep_blank_values=True,
        )

        self.assertEqual(len(RANK_BOARDS), 8)
        self.assertEqual(set(TODAY_RELEASE_TYPES), {
            "playlet", "comic_series_rank", "ai_playlet"
        })
        self.assertEqual(first["selected_items"], ["all"])
        self.assertEqual(first["client_template"], ["2"])
        self.assertEqual(first["client_req_type"], ["2"])
        self.assertEqual(first["unlimited_selector_change_type"], ["2"])
        self.assertNotIn("offset", first)
        self.assertEqual(later["unlimited_selector_change_type"], ["1"])
        self.assertEqual(later["offset"], ["15"])
        self.assertEqual(later["session_id"], ["rotated"])
        self.assertEqual(later["rank_version"], ["version-2"])
        self.assertEqual(later["filter_ids"], ["a,b"])

    def test_rank_parser_preserves_selector_type_and_paging_state(self):
        page = parse_rank_page(
            {
                "data": {
                    "cell_view": {
                        "cell_data": [
                            {
                                "video_data": [
                                    {
                                        "series_id": "comic-1",
                                        "title": "漫剧一",
                                        "content_type": 1004,
                                        "sub_title": "校园",
                                    }
                                ]
                            },
                            {"video_data": [{"series_id": "comic-1", "title": "重复"}]},
                        ]
                    },
                    "next_offset": 15,
                    "session_id": "next-session",
                    "rank_version": "rank-v2",
                    "has_more": True,
                }
            },
            release_type="comic_series_rank",
        )

        self.assertEqual(len(page["items"]), 1)
        self.assertEqual(page["items"][0]["release_type"], "comic_series_rank")
        self.assertEqual(page["items"][0]["content_type"], 1004)
        self.assertEqual(page["items"][0]["category_tags"], ["校园"])
        self.assertEqual(page["next_offset"], 15)
        self.assertEqual(page["rank_version"], "rank-v2")

    def test_batch_metrics_parser_maps_each_series_without_missing_to_zero(self):
        metrics = parse_batch_metrics(
            {
                "data": {
                    "comic-1": {
                        "video_data": {
                            "series_id": "comic-1",
                            "create_time": 1_788_408_000,
                            "series_play_cnt": 0,
                            "followed_cnt": 4,
                        }
                    },
                    "ai-1": {
                        "video_data": {
                            "series_id": "ai-1",
                            "create_time": 1_788_408_001,
                        }
                    },
                }
            }
        )

        self.assertEqual(metrics["comic-1"]["play_count"], 0)
        self.assertEqual(metrics["comic-1"]["collect_count"], 4)
        self.assertIsNone(metrics["comic-1"]["hot_count"])
        self.assertIsNone(metrics["ai-1"]["play_count"])


if __name__ == "__main__":
    unittest.main()

class FeedDisplayRegressionTests(unittest.TestCase):
    def test_type_specific_rank_board_ids(self):
        for kind, board, selected, sub in [
            ('comic_series_rank', 'ranklist_hot_sc', 'comic_series_rank', 'comic_series_hot_rank'),
            ('ai_playlet', 'ranklist_new_rank_sc', 'ai_playlet', 'ai_playlet_new_rank'),
            ('human', 'ranklist_followed', 'human', 'human_followed'),
            ('comic_series_rank', 'ranklist_followed', 'all', 'ranklist_followed'),
        ]:
            query = parse_qs(urlparse(build_rank_url('device', kind, board)).query)
            self.assertEqual(query['selected_items'], [selected])
            self.assertEqual(query['sub_selected_items'], [sub])

    def test_genres_exclude_episode_counts_and_cast(self):
        page = parse_rank_page({'data': {'cell_view': {'video_data': [{
            'series_id': 'comic', 'content_type': 1004,
            'sub_title': '东方仙侠·全300集·演员甲',
            'sub_title_list': [{'content': '萌宝', 'data_type': 3}, {'content': '演员甲', 'data_type': 23}],
        }]}}})
        self.assertEqual(page['items'][0]['category_tags'], ['萌宝', '东方仙侠'])
        self.assertEqual(page['items'][0]['release_type'], 'comic_series_rank')
        self.assertEqual(page['items'][0]['sub_title'], '东方仙侠·全300集·演员甲')

    def test_preserves_captured_completion_label_without_polluting_genres(self):
        # /duanju/search raw, 2026-09-13, series 7683098724637101080.
        page = parse_rank_page({'data': {'cell_view': {'video_data': [{
            'series_id': '7683098724637101080', 'content_type': 1,
            'sub_title': '都市日常·都市脑洞·全64集', 'episode_cnt': 64,
            'sub_title_list': [
                {'content': '都市日常', 'data_type': 3},
                {'content': '都市脑洞', 'data_type': 3},
                {'content': '全64集', 'data_type': 13},
            ],
        }]}}})
        item = page['items'][0]
        self.assertEqual(item['sub_title'], '都市日常·都市脑洞·全64集')
        self.assertEqual(item['sub_title_list'], [{'content': '全64集'}])
        self.assertEqual(item['category_tags'], ['都市日常', '都市脑洞'])

    def test_preserves_explicit_serial_label_but_does_not_invent_completion(self):
        page = parse_rank_page({'data': {'cell_view': {'video_data': [{
            'series_id': 'serial', 'content_type': 1, 'episode_cnt': 80,
            'sub_title_list': [{'content': '连载中', 'data_type': 13}],
            'secondary_info_list': [{'content': '连载中'}],
        }, {
            'series_id': 'unknown', 'content_type': 1, 'episode_cnt': 80,
            'video_desc': '故事已经完结，全80集', 'status': 2, 'creation_status': 0,
        }]}}})
        self.assertEqual(page['items'][0]['sub_title_list'], [{'content': '连载中'}])
        self.assertEqual(page['items'][1]['sub_title_list'], [])
        self.assertEqual(page['items'][1]['sub_title'], '')
        self.assertNotIn('complete', page['items'][1])

    def test_preserves_ai_type_and_known_counters(self):
        page = parse_rank_page({'data': {'cell_view': {'video_data': [{
            'series_id': 'ai', 'content_type': 1004, 'video_category_type': 'ai_video',
            'hot_score': 12500, 'comment_count': 42,
        }]}}})
        self.assertEqual(page['items'][0]['release_type'], 'ai_playlet')
        self.assertEqual(page['items'][0]['hot_count'], 12500)
        self.assertEqual(page['items'][0]['comment_count'], 42)

class TextMetricTests(unittest.TestCase):
    def test_parses_heat_text_without_using_recommendation_or_play_count(self):
        for label, expected in [('10536万热度',105360000),('2.5亿热度',250000000),('996万推荐',None),('1870万收藏',None)]:
            page = parse_rank_page({'data': {'cell_view': {'video_data': [{
                'series_id': 'a', 'play_cnt': 123, 'rec_text_item': {'RecommendText': label}
            }]}}})
            self.assertEqual(page['items'][0]['hot_count'], expected)

class RecommendationGenreTests(unittest.TestCase):
    def test_recommendation_genre_tags_exclude_promotion_tags(self):
        from core.duanju_feeds import series_genres
        self.assertEqual(series_genres({'rec_tags': [
            {'content': '玄幻', 'data_type': 1, 'rec_type': 24},
            {'content': '系统', 'data_type': 1, 'rec_type': 24},
            {'content': '点赞破100万', 'data_type': 1, 'rec_type': 10},
        ]}), ['玄幻', '系统'])


class HomepageHeatTests(unittest.TestCase):
    def test_homepage_rec_text_and_next_page_keep_real_heat(self):
        from endpoints.duanju import _parse_bookmall_tab, _parse_bookmall_change
        videos = [
            {"series_id": "live", "content_type": 1, "rec_text": "6130万热度"},
            {"series_id": "comic", "content_type": 1004, "rec_text": "10122万热度"},
            {"series_id": "ai", "video_category_type": "ai_video", "rec_text": "🔥 1.25亿热度"},
            {"series_id": "recommend", "rec_text": "996万推荐", "play_cnt": 999},
            {"series_id": "zero", "hot_score": 0, "rec_text": "6130万热度"},
        ]
        cell = {"cell_data": [{"video_data": [video]} for video in videos]}
        first = _parse_bookmall_tab({"data": {"tab_item": [{"tab_type": "38", "cell_data": [cell]}]}}, "38")
        more = _parse_bookmall_change({"data": {"cell_view": cell}})
        for page in (first, more):
            self.assertEqual([item["hot_count"] for item in page["items"]], [61300000, 101220000, 125000000, None, 0])
