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
