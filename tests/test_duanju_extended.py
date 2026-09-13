import asyncio
import json
import unittest
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import parse_qs, urlparse
from zoneinfo import ZoneInfo

from fastapi import FastAPI
from starlette.requests import Request

from endpoints.duanju import (
    _duanju_new_release_cache,
    _duanju_rank_page_cache,
    _duanju_video_cache,
    _category_groups,
    _fetch_new_release_page,
    _fetch_rank_feed_page,
    _fetch_video_model,
    _fetch_series_metrics,
    _new_release_candidates,
    _series_item,
    _series_metadata_body,
    _pick_source,
    duanju_categories,
    duanju_new_releases,
    duanju_rank,
    duanju_series_metrics,
)


SHANGHAI = ZoneInfo("Asia/Shanghai")


def request_for(client, path="/api/duanju/new-releases"):
    app = FastAPI()
    app.state.client = client
    scope = {
        "type": "http",
        "method": "GET",
        "scheme": "http",
        "server": ("test", 80),
        "client": ("test", 123),
        "root_path": "",
        "path": path,
        "raw_path": path.encode(),
        "query_string": b"",
        "headers": [],
        "app": app,
    }
    return Request(scope)


def response_json(response):
    return json.loads(response.body.decode("utf-8"))


class RecordingClient:
    def __init__(self, handler):
        self.handler = handler
        self.calls = []

    async def call_with_device(
        self,
        url_builder,
        method="GET",
        data="",
        aid=1967,
        max_device_retries=3,
        need_key=False,
        content_type="application/x-www-form-urlencoded",
        preferred_device_id=None,
        return_device_id=False,
    ):
        selected_device_id = preferred_device_id or "device-under-test"
        url = url_builder(selected_device_id)
        self.calls.append(
            {
                "url": url,
                "method": method,
                "data": data,
                "aid": aid,
                "max_device_retries": max_device_retries,
                "need_key": need_key,
                "content_type": content_type,
                "preferred_device_id": preferred_device_id,
                "return_device_id": return_device_id,
            }
        )
        result = await self.handler(
            url=url,
            method=method,
            data=data,
            aid=aid,
            preferred_device_id=preferred_device_id,
        )
        if result.get("ok") and return_device_id:
            result["device_id"] = selected_device_id
        return result


class DuanjuExtendedTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        _duanju_new_release_cache.clear()
        _duanju_rank_page_cache.clear()

    def test_auto_definition_prefers_1080_then_720(self):
        def source(definition):
            return {
                "definition": definition,
                "urls": [f"https://example.invalid/{definition}"],
                "spade_a": "safe-test-value",
            }

        self.assertEqual(
            _pick_source([source("720p"), source("1080p")], "auto")["definition"],
            "1080p",
        )
        self.assertEqual(
            _pick_source([source("720p")], "auto")["definition"], "720p"
        )

    async def test_video_model_uses_device_failover_client(self):
        _duanju_video_cache.clear()

        class Client:
            def __init__(self):
                self.calls = []

            async def call_with_device(self, url_builder, **kwargs):
                self.calls.append({"url": url_builder("device-under-test"), **kwargs})
                return {
                    "ok": True,
                    "upstream": {
                        "data": {
                            "episode-1": {
                                "video_model": json.dumps({
                                    "video_list": {
                                        "720p": {
                                            "main_url": "https://cdn.example.test/video",
                                            "encrypt_info": {"spade_a": "spade"},
                                        },
                                    },
                                }),
                            },
                        },
                    },
                }

        client = Client()
        request = SimpleNamespace(app=SimpleNamespace(state=SimpleNamespace(client=client)))

        result = await _fetch_video_model(request, "episode-1")

        self.assertEqual(result["sources"][0]["definition"], "720p")
        self.assertEqual(client.calls[0]["method"], "POST")
        self.assertEqual(client.calls[0]["aid"], 8662)
        self.assertEqual(client.calls[0]["content_type"], "application/json")

    def test_category_groups_keep_upstream_order_and_deduplicate(self):
        selector = {
            "outer_row": {
                "row_name": "综合",
                "items": [
                    {"selector_item_id": "all", "show_name": "全部"},
                    {"selector_item_id": "all", "show_name": "重复"},
                ],
            },
            "inner_rows": [
                {
                    "row_name": "时代背景",
                    "items": [
                        {"selector_item_id": "cate_4", "show_name": "校园"}
                    ],
                },
                {
                    "row_name": "角色设定",
                    "items": [
                        {"selector_item_id": "cate_20", "show_name": "神豪"}
                    ],
                },
            ],
        }

        groups = _category_groups(selector)

        self.assertEqual([group["name"] for group in groups], ["综合", "时代背景", "角色设定"])
        self.assertEqual(groups[0]["items"], [{"id": "all", "name": "全部"}])
        self.assertEqual(groups[1]["items"][0]["id"], "cate_4")

    async def test_categories_endpoint_returns_grouped_selector_without_raw_data(self):
        selector = {
            "outer_row": {
                "row_name": "综合",
                "items": [{"selector_item_id": "all", "show_name": "全部"}],
            },
            "inner_rows": [
                {
                    "row_name": "主题情节",
                    "items": [
                        {"selector_item_id": "cate_9", "show_name": "先婚后爱"}
                    ],
                }
            ],
        }

        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "tab_item": [
                            {
                                "tab_type": 38,
                                "cell_data": [{"cell_selector": selector}],
                            }
                        ]
                    }
                },
            }

        client = RecordingClient(handler)
        response = await duanju_categories(
            request_for(client, "/api/duanju/categories"), content_type="drama"
        )
        payload = response_json(response)

        self.assertEqual(payload["code"], 0)
        self.assertEqual(payload["data"]["groups"][1]["name"], "主题情节")
        self.assertNotIn("raw", json.dumps(payload))
        self.assertEqual(client.calls[0]["aid"], 8662)

    def test_metadata_body_contains_only_stable_business_fields(self):
        body = json.loads(_series_metadata_body("s1", 1004))

        self.assertEqual(body["series_id"], "s1")
        self.assertEqual(body["content_type"], 1004)
        self.assertFalse(body["biz_param"]["disable_digg_stat"])
        self.assertNotIn("device_id", body)
        self.assertNotIn("signature", body)

    def test_new_candidate_parser_keeps_content_type_and_normalized_fields(self):
        upstream = {
            "data": {
                "cell_view": {
                    "cell_data": [
                        {
                            "video_data": [
                                {
                                    "series_id": "s1",
                                    "title": "新剧",
                                    "content_type": 1004,
                                    "cover": "https://example.invalid/cover",
                                    "episode_cnt": 60,
                                }
                            ]
                        }
                    ]
                }
            }
        }

        item = _new_release_candidates(upstream)[0]

        self.assertEqual(item["series_id"], "s1")
        self.assertEqual(item["book_id"], "s1")
        self.assertEqual(item["content_type"], 1004)
        self.assertEqual(item["episode_count"], 60)

    def test_all_catalog_normalizers_keep_numeric_content_type(self):
        item = _series_item({"series_id": "s1", "content_type": 1004})

        self.assertEqual(item["content_type"], 1004)

    async def test_metadata_fetch_uses_signed_device_failover_post(self):
        async def handler(**call):
            body = json.loads(call["data"])
            self.assertEqual(call["method"], "POST")
            self.assertEqual(call["aid"], 8662)
            self.assertEqual(body["series_id"], "s1")
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "series": {
                            "series_id": "s1",
                            "create_time": 123,
                            "series_play_cnt": 0,
                            "hot_score": 5,
                            "followed_cnt": 6,
                            "digg_cnt": 7,
                            "comment_cnt": 12,
                        }
                    }
                },
            }

        client = RecordingClient(handler)

        metrics = await _fetch_series_metrics(client, "s1", 1004)

        self.assertEqual(metrics["play_count"], 0)
        self.assertEqual(metrics["like_count"], 7)
        self.assertEqual(metrics["comment_count"], 12)
        self.assertEqual(len(client.calls), 1)
        self.assertEqual(client.calls[0]["content_type"], "application/json")
        self.assertNotIn("device-under-test", client.calls[0]["data"])

    async def test_series_metrics_endpoint_returns_only_normalized_fields(self):
        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "series": {
                            "series_id": "endpoint-series",
                            "create_time": 123,
                            "series_play_cnt": 10,
                            "hot_score": 20,
                            "followed_cnt": 30,
                            "digg_cnt": 40,
                            "private_debug": "must-not-leak",
                        }
                    }
                },
            }

        response = await duanju_series_metrics(
            request_for(RecordingClient(handler), "/api/duanju/series-metrics"),
            series_id="endpoint-series",
            content_type=1004,
        )
        payload = response_json(response)

        self.assertEqual(payload["code"], 0)
        self.assertEqual(
            set(payload["data"]),
            {
                "series_id",
                "content_type",
                "online_time",
                "play_count",
                "hot_count",
                "collect_count",
                "like_count",
                "comment_count",
            },
        )
        self.assertNotIn("private_debug", json.dumps(payload))

    async def test_playlet_release_endpoint_uses_direct_date_feed_and_buffers_overfetch(self):
        today_noon = int(
            datetime.now(SHANGHAI).replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
        )

        async def handler(**call):
            query = parse_qs(urlparse(call["url"]).query)
            offset = int(query.get("offset", ["0"])[0])
            start = 1 if offset == 0 else 16
            rows = [
                {
                    "is_online": True,
                    "schedule_publish_time": today_noon + index,
                    "sub_title_list": ["校园"],
                    "subscribe_data": {
                        "series_id": f"release-{index}",
                        "title": f"新剧 {index}",
                        "content_type": 1,
                        "episode_cnt": 60,
                        "video_detail": {
                            "series_id": f"release-{index}",
                            "series_play_cnt": index,
                            "followed_cnt": index * 3,
                        },
                    },
                }
                for index in range(start, start + 15)
            ]
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "subscribe_items": rows,
                        "next_offset": offset + 15,
                        "has_more": offset == 0,
                        "session_id": f"session-{offset + 15}",
                    }
                },
            }

        client = RecordingClient(handler)
        response = await duanju_new_releases(
            request_for(client), release_type="playlet", cursor="", limit=20
        )
        payload = response_json(response)

        self.assertEqual(payload["code"], 0)
        self.assertEqual(len(payload["data"]["items"]), 20)
        self.assertEqual(len({item["series_id"] for item in payload["data"]["items"]}), 20)
        self.assertTrue(payload["data"]["has_more"])
        self.assertNotIn("raw", json.dumps(payload))
        self.assertEqual(len(client.calls), 2)
        self.assertTrue(all(call["method"] == "GET" for call in client.calls))
        self.assertTrue(all(call["aid"] == 8662 for call in client.calls))
        first_query = parse_qs(urlparse(client.calls[0]["url"]).query)
        self.assertEqual(first_query["target_date"], [datetime.now(SHANGHAI).strftime("%Y%m%d")])
        self.assertEqual(first_query["tab_type"], ["5"])

    async def test_playlet_release_page_carries_rotating_session_and_device(self):
        async def handler(**call):
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            self.assertEqual(call["aid"], 8662)
            self.assertEqual(query["target_date"], ["20260903"])
            self.assertEqual(query["offset"], ["30"])
            self.assertEqual(query["session_id"], ["rotated-session"])
            self.assertEqual(call["preferred_device_id"], "sticky-device")
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "subscribe_items": [],
                        "next_offset": 45,
                        "has_more": False,
                        "session_id": "next-session",
                    }
                },
            }

        client = RecordingClient(handler)
        page = await _fetch_new_release_page(
            client,
            "playlet",
            {
                "offset": 30,
                "session_id": "rotated-session",
                "device_id": "sticky-device",
            },
            target_date="20260903",
        )

        self.assertEqual(
            page["next"],
            {
                "offset": 45,
                "session_id": "next-session",
                "rank_version": "",
                "filter_ids": [],
                "device_id": "sticky-device",
            },
        )

    async def test_typed_release_batches_metadata_and_keeps_selector_identity(self):
        today_noon = int(
            datetime.now(SHANGHAI).replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
        )

        async def handler(**call):
            if call["method"] == "POST":
                body = json.loads(call["data"])
                self.assertEqual(body["series_id"], "typed-1,typed-2")
                return {
                    "ok": True,
                    "upstream": {
                        "data": {
                            series_id: {
                                "video_data": {
                                    "series_id": series_id,
                                    "create_time": today_noon + index,
                                    "series_play_cnt": index,
                                }
                            }
                            for index, series_id in enumerate(("typed-1", "typed-2"), 1)
                        }
                    },
                }
            query = parse_qs(urlparse(call["url"]).query)
            self.assertEqual(query["selected_items"], ["ai_playlet"])
            self.assertEqual(query["sub_selected_items"], ["ai_playlet_new_rank"])
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {
                            "cell_data": [{
                                "video_data": [
                                    {
                                        "series_id": "typed-1",
                                        "title": "AI一",
                                        "content_type": 1004,
                                        "video_category_type": "ai_video",
                                    },
                                    {
                                        "series_id": "typed-2",
                                        "title": "AI二",
                                        "content_type": 1004,
                                        "video_category_type": "ai_video",
                                    },
                                ]
                            }]
                        },
                        "next_offset": 2,
                        "has_more": False,
                    }
                },
            }

        client = RecordingClient(handler)
        response = await duanju_new_releases(
            request_for(client), release_type="ai_playlet", cursor="", limit=20
        )
        payload = response_json(response)

        self.assertEqual(payload["code"], 0)
        self.assertEqual([item["release_type"] for item in payload["data"]["items"]], ["ai_playlet", "ai_playlet"])
        self.assertEqual([call["method"] for call in client.calls], ["GET", "POST"])
        self.assertTrue(all(call["aid"] == 8662 for call in client.calls))

    async def test_release_page_rejects_non_advancing_offset(self):
        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "subscribe_items": [],
                        "next_offset": 30,
                        "session_id": "different-session",
                        "has_more": True,
                    }
                },
            }

        with self.assertRaisesRegex(RuntimeError, "上游分页 offset 未前进"):
            await _fetch_new_release_page(
                RecordingClient(handler),
                "playlet",
                {"offset": 30, "session_id": "old-session"},
                target_date="20260903",
            )

    async def test_playlet_repeated_terminal_page_keeps_new_rows_then_ends(self):
        today_noon = int(
            datetime.now(SHANGHAI).replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
        )

        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "subscribe_items": [
                            {
                                "is_online": True,
                                "schedule_publish_time": today_noon,
                                "subscribe_data": {
                                    "series_id": "release-2",
                                    "title": "终页新增剧",
                                    "content_type": 1,
                                },
                            }
                        ],
                        "next_offset": 30,
                        "session_id": "same-session",
                        "has_more": True,
                    }
                },
            }

        page = await _fetch_new_release_page(
            RecordingClient(handler),
            "playlet",
            {
                "offset": 30,
                "session_id": "same-session",
                "filter_ids": ["release-1"],
            },
            target_date=datetime.now(SHANGHAI).strftime("%Y%m%d"),
        )

        self.assertFalse(page["has_more"])
        self.assertEqual(page["next"]["offset"], 30)
        self.assertEqual([item["series_id"] for item in page["items"]], ["release-2"])

    async def test_rank_endpoint_aggregates_twenty_and_returns_eight_boards(self):
        calls = 0

        async def handler(**call):
            nonlocal calls
            calls += 1
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            self.assertEqual(call["aid"], 8662)
            self.assertEqual(query["selected_items"], ["all"])
            self.assertEqual(query["client_template"], ["2"])
            self.assertEqual(query["client_req_type"], ["2"])
            self.assertEqual(query["unlimited_selector_change_type"], ["2" if calls == 1 else "1"])
            start = 1 if calls == 1 else 16
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {"series_id": f"rank-{index}", "title": f"榜单{index}"}
                            for index in range(start, start + 15)
                        ]}]},
                        "next_offset": calls * 15,
                        "has_more": calls == 1,
                        "session_id": f"session-{calls}",
                        "rank_version": f"version-{calls}",
                    }
                },
            }

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler), "/api/duanju/rank"),
            board="ranklist_must_watch",
            cursor="",
            limit=20,
        ))

        self.assertEqual(payload["code"], 0)
        self.assertEqual(len(payload["data"]["items"]), 20)
        self.assertEqual(len(payload["data"]["boards"]), 8)
        self.assertEqual(payload["data"]["board"], "ranklist_must_watch")
        self.assertTrue(payload["data"]["next_cursor"])

    async def test_rank_endpoint_uses_requested_release_type(self):
        async def handler(**call):
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            self.assertEqual(query["selected_items"], ["ai_playlet"])
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {
                                "series_id": "ai-rank-1",
                                "title": "AI榜单项",
                                "content_type": 1004,
                                "video_category_type": "ai_video",
                            }
                        ]}]},
                        "next_offset": 1,
                        "has_more": False,
                    }
                },
            }

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler), "/api/duanju/rank"),
            board="ranklist_new_rank_sc",
            release_type="ai_playlet",
            cursor="",
            limit=20,
        ))

        self.assertEqual(payload["code"], 0)
        self.assertEqual(payload["data"]["release_type"], "ai_playlet")
        self.assertEqual(payload["data"]["items"][0]["release_type"], "ai_playlet")

    async def test_rank_endpoint_filters_ai_by_upstream_video_category_type(self):
        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {
                                "series_id": "ai-1",
                                "title": "AI剧",
                                "content_type": 1004,
                                "video_category_type": "ai_video",
                            },
                            {
                                "series_id": "ordinary-1",
                                "title": "普通剧",
                                "content_type": 1,
                            },
                        ]}]},
                        "next_offset": 2,
                        "has_more": False,
                    }
                },
            }

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler), "/api/duanju/rank"),
            board="ranklist_hot_sc",
            release_type="ai_playlet",
            cursor="",
            limit=20,
        ))

        self.assertEqual([item["series_id"] for item in payload["data"]["items"]], ["ai-1"])

    async def test_rank_endpoint_maps_human_release_type_to_upstream_selector(self):
        async def handler(**call):
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            self.assertEqual(query["selected_items"], ["human"])
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {"series_id": "human-rank-1", "title": "真人榜单项", "content_type": 1}
                        ]}]},
                        "next_offset": 1,
                        "has_more": False,
                    }
                },
            }

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler), "/api/duanju/rank"),
            board="ranklist_new_rank_sc",
            release_type="playlet",
            cursor="",
            limit=20,
        ))

        self.assertEqual(payload["code"], 0)
        self.assertEqual(payload["data"]["items"][0]["series_id"], "human-rank-1")

    async def test_rank_endpoint_filters_mixed_upstream_rows_by_content_type(self):
        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {"series_id": "drama-1", "title": "真人榜单项", "content_type": 1},
                            {"series_id": "manju-1", "title": "漫剧榜单项", "content_type": 1004},
                        ]}]},
                        "next_offset": 2,
                        "has_more": False,
                    }
                },
            }

        client = RecordingClient(handler)
        drama = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_hot_sc", release_type="playlet", cursor="", limit=20,
        ))
        manju = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_hot_sc", release_type="comic_series_rank", cursor="", limit=20,
        ))

        self.assertEqual([item["series_id"] for item in drama["data"]["items"]], ["drama-1"])
        self.assertEqual([item["series_id"] for item in manju["data"]["items"]], ["manju-1"])

    async def test_rank_endpoint_falls_back_to_all_when_typed_selector_is_empty(self):
        calls = 0

        async def handler(**call):
            nonlocal calls
            calls += 1
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            selected = query["selected_items"][0]
            if calls == 1:
                self.assertEqual(selected, "comic_series_rank")
                items = []
            else:
                self.assertEqual(selected, "all")
                items = [
                    {"series_id": "hot-search-manju", "title": "热搜漫剧", "content_type": 1004},
                    {"series_id": "hot-search-drama", "title": "热搜真人剧", "content_type": 1},
                ]
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": items}]},
                        "next_offset": calls,
                        "has_more": False,
                    }
                },
            }

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler), "/api/duanju/rank"),
            board="ranklist_hot_search_sc",
            release_type="comic_series_rank",
            cursor="",
            limit=20,
        ))

        self.assertEqual(calls, 2)
        self.assertEqual([item["series_id"] for item in payload["data"]["items"]], ["hot-search-manju"])

    async def test_rank_cursor_carries_state_and_reuses_bound_device(self):
        calls = 0

        async def handler(**call):
            nonlocal calls
            calls += 1
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            if calls == 2:
                self.assertEqual(call["preferred_device_id"], "device-under-test")
                self.assertEqual(query["offset"], ["15"])
                self.assertEqual(query["session_id"], ["sticky-session"])
                self.assertEqual(query["rank_version"], ["sticky-version"])
                self.assertEqual(query["filter_ids"], [",".join(f"sticky-{index}" for index in range(1, 16))])
            start = 1 if calls == 1 else 16
            count = 15 if calls == 1 else 5
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": [
                            {"series_id": f"sticky-{index}", "title": "榜单项"}
                            for index in range(start, start + count)
                        ]}]},
                        "next_offset": 15 if calls == 1 else 20,
                        "has_more": calls == 1,
                        "session_id": "sticky-session" if calls == 1 else "done",
                        "rank_version": "sticky-version",
                    }
                },
            }

        client = RecordingClient(handler)
        first = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_hot_sc", cursor="", limit=10,
        ))["data"]
        second = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_hot_sc", cursor=first["next_cursor"], limit=10,
        ))["data"]

        self.assertEqual(calls, 2)
        self.assertEqual(len(first["items"]), 10)
        self.assertEqual(len(second["items"]), 10)
        self.assertEqual(second["items"][-1]["series_id"], "sticky-20")
        self.assertFalse(second["has_more"])

    async def test_rank_cursor_is_bound_to_its_board(self):
        async def handler(**_call):
            return {
                "ok": True,
                "upstream": {"data": {
                    "cell_view": {"cell_data": [{"video_data": [{"series_id": "one"}]}]},
                    "next_offset": 1, "has_more": True, "session_id": "s",
                }},
            }

        client = RecordingClient(handler)
        first = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_hot_sc", cursor="", limit=1,
        ))["data"]
        wrong = response_json(await duanju_rank(
            request_for(client, "/api/duanju/rank"),
            board="ranklist_followed", cursor=first["next_cursor"], limit=1,
        ))

        self.assertEqual(wrong["code"], -2)

    async def test_typed_new_release_uses_new_rank_selector(self):
        captured = []

        async def handler(**call):
            captured.append(call["url"])
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": []},
                        "next_offset": 0,
                        "has_more": False,
                    }
                },
            }

        response = await duanju_new_releases(
            request_for(RecordingClient(handler)),
            release_type="ai_playlet",
            cursor="",
            limit=20,
        )

        self.assertEqual(response_json(response)["code"], 0)
        query = parse_qs(urlparse(captured[0]).query)
        self.assertEqual(query["selected_items"], ["ai_playlet"])
        self.assertEqual(query["sub_selected_items"], ["ai_playlet_new_rank"])

    async def test_typed_new_release_falls_back_to_all_when_selector_is_empty(self):
        calls = []
        today_noon = int(
            datetime.now(SHANGHAI).replace(hour=12, minute=0, second=0, microsecond=0).timestamp()
        )

        async def handler(**call):
            calls.append(call)
            if call["method"] == "POST":
                return {
                    "ok": True,
                    "upstream": {
                        "data": {
                            "new-ai-1": {
                                "video_data": {
                                    "series_id": "new-ai-1",
                                    "create_time": today_noon,
                                }
                            }
                        }
                    },
                }
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            selected = query["selected_items"][0]
            items = [] if selected == "ai_playlet" else [{
                "series_id": "new-ai-1",
                "title": "今日 AI 剧",
                "content_type": 1004,
                "video_category_type": "ai_video",
            }]
            return {
                "ok": True,
                "upstream": {
                    "data": {
                        "cell_view": {"cell_data": [{"video_data": items}]},
                        "next_offset": 1,
                        "has_more": False,
                    }
                },
            }

        page = await _fetch_new_release_page(
            RecordingClient(handler),
            "ai_playlet",
            0,
            target_date=datetime.now(SHANGHAI).strftime("%Y%m%d"),
        )

        selected_items = [
            parse_qs(urlparse(call["url"]).query)["selected_items"][0]
            for call in calls
            if call["method"] == "GET"
        ]
        self.assertEqual(selected_items, ["ai_playlet", "all"])
        self.assertEqual([item["series_id"] for item in page["items"]], ["new-ai-1"])

    async def test_rank_fallback_restarts_mixed_source_without_skipping_its_first_page(self):
        async def handler(**call):
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            selector = query["selected_items"][0]
            offset = int(query.get("offset", ["0"])[0])
            if selector == "comic_series_rank":
                items = [{"series_id": "typed-first", "content_type": 1004}] if offset == 0 else []
                next_offset, has_more = 15, offset == 0
            else:
                self.assertEqual(query["unlimited_selector_change_type"], ["2"])
                self.assertNotIn("offset", query)
                self.assertNotIn("session_id", query)
                self.assertEqual(query["rank_version"], [""])
                self.assertNotIn("filter_ids", query)
                self.assertEqual(call["preferred_device_id"], "device-under-test")
                items = [
                    {"series_id": "typed-first", "content_type": 1004},
                    {"series_id": "mixed-first", "content_type": 1004},
                ]
                next_offset, has_more = 2, False
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": items},
                "next_offset": next_offset, "has_more": has_more,
                "session_id": "typed-session", "rank_version": "typed-version",
            }}}

        payload = response_json(await duanju_rank(
            request_for(RecordingClient(handler)), board="ranklist_hot_sc",
            release_type="comic_series_rank", cursor="", limit=20,
        ))

        self.assertEqual([item["series_id"] for item in payload["data"]["items"]], [
            "typed-first", "mixed-first",
        ])

    async def test_new_release_fallback_restarts_mixed_source_paging(self):
        async def handler(**call):
            if call["method"] == "POST":
                return {"ok": True, "upstream": {"data": {}}}
            query = parse_qs(urlparse(call["url"]).query, keep_blank_values=True)
            if query["selected_items"] == ["comic_series_rank"]:
                items, next_offset, has_more = [], 30, False
            else:
                self.assertNotIn("offset", query)
                self.assertNotIn("session_id", query)
                self.assertNotIn("filter_ids", query)
                self.assertEqual(query["rank_version"], [""])
                self.assertEqual(query["unlimited_selector_change_type"], ["2"])
                items = [{"series_id": "mixed-first", "content_type": 1004}]
                next_offset, has_more = 1, True
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": items},
                "next_offset": next_offset, "has_more": has_more,
            }}}

        page = await _fetch_new_release_page(
            RecordingClient(handler), "comic_series_rank",
            {"offset": 15, "session_id": "typed-session", "rank_version": "typed-version",
             "filter_ids": ["typed-first"], "device_id": "sticky-device"},
            target_date="20260909",
        )

        self.assertEqual([item["series_id"] for item in page["items"]], ["mixed-first"])
        self.assertEqual(page["next"]["offset"], 1)
        self.assertEqual(page["next"]["selector_type"], "all")
        self.assertTrue(page["has_more"])

    async def test_new_release_page_cache_keeps_devices_separate(self):
        async def handler(**_call):
            return {"ok": True, "upstream": {"data": {
                "subscribe_items": [], "next_offset": 30, "has_more": False,
            }}}

        client = RecordingClient(handler)
        for device in ("device-a", "device-b"):
            page = await _fetch_new_release_page(
                client, "playlet", {"offset": 15, "device_id": device},
                target_date="20260909",
            )
            self.assertEqual(page["next"]["device_id"], device)
        self.assertEqual(len(client.calls), 2)

    async def test_new_release_metadata_failure_keeps_known_series(self):
        async def handler(**call):
            if call["method"] == "POST":
                return {"ok": False, "msg": "metadata temporarily unavailable"}
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{
                    "series_id": "known-comic", "title": "已知新剧", "content_type": 1004,
                    "video_detail": {"series_play_cnt": 7},
                }]},
                "next_offset": 1, "has_more": False,
            }}}

        with self.assertLogs("fanqie.duanju", level="WARNING"):
            payload = response_json(await duanju_new_releases(
                request_for(RecordingClient(handler)), release_type="comic_series_rank",
                cursor="", limit=20,
            ))

        self.assertEqual(payload["code"], 0)
        item = payload["data"]["items"][0]
        self.assertEqual(item["series_id"], "known-comic")
        self.assertEqual(item["play_count"], 7)
        self.assertIsNone(item["online_time"])

    async def test_new_release_response_distinguishes_daily_schedule_from_latest_rank(self):
        async def handler(**_call):
            return {"ok": True, "upstream": {"data": {
                "subscribe_items": [], "cell_view": {}, "next_offset": 0, "has_more": False,
            }}}

        for release_type, source, date_scope in (
            ("playlet", "subscribe", "today"),
            ("comic_series_rank", "rank", "latest"),
            ("ai_playlet", "rank", "latest"),
        ):
            with self.subTest(release_type=release_type):
                data = response_json(await duanju_new_releases(
                    request_for(RecordingClient(handler)), release_type=release_type,
                    cursor="", limit=20,
                ))["data"]
                self.assertEqual(data.get("source"), source)
                self.assertEqual(data.get("date_scope"), date_scope)
                self.assertEqual(data["date"], datetime.now(SHANGHAI).date().isoformat())

    async def test_recent_live_action_checks_each_calendar_day_and_preserves_inner_cursor(self):
        from unittest.mock import patch
        from datetime import timedelta
        calls = []
        current = datetime.now(SHANGHAI)
        async def fetch(_client, release_type, state, *, target_date):
            calls.append((target_date, state))
            return {"items": [], "next": "second" if state is None else None, "has_more": state is None}
        with patch("endpoints.duanju._fetch_new_release_page", side_effect=fetch):
            data = response_json(await duanju_new_releases(request_for(None), release_type="playlet", cursor="", limit=20, days=3))["data"]
        self.assertFalse(data["has_more"])
        self.assertEqual(data["date_scope"], "recent")
        self.assertEqual(calls, [((current - timedelta(days=i)).strftime("%Y%m%d"), state) for i in range(3) for state in [None, "second"]])

    async def test_rank_reuses_recent_pages(self):
        async def handler(**_call):
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": "cached-rank", "title": "榜单"}]},
                "next_offset": 1, "has_more": False,
            }}}

        client = RecordingClient(handler)
        first = response_json(await duanju_rank(
            request_for(client), board="ranklist_must_watch", release_type="all", cursor="", limit=20,
        ))["data"]
        first["items"][0]["title"] = "caller modified"
        second = response_json(await duanju_rank(
            request_for(client), board="ranklist_must_watch", release_type="all", cursor="", limit=20,
        ))["data"]

        self.assertEqual(second["items"][0]["title"], "榜单")
        self.assertEqual(len(client.calls), 1)

    async def test_rank_cache_isolates_source_date_device_and_paging_state(self):
        calls = 0

        async def handler(**_call):
            nonlocal calls
            calls += 1
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": f"page-{calls}"}]},
                "next_offset": 30, "has_more": False,
            }}}

        client = RecordingClient(handler)
        base = {
            "selector_type": "all", "board": "ranklist_hot_sc", "state": None,
            "target_date": "20260909", "preferred_device_id": "device-a",
        }
        first = await _fetch_rank_feed_page(client, **base)
        first["items"][0]["title"] = "mutated caller item"
        for change in (
            {"selector_type": "human"},
            {"board": "ranklist_new_rank_sc"},
            {"target_date": "20260910"},
            {"preferred_device_id": "device-b"},
            {"state": {"offset": 15}},
            {"state": {"offset": 15, "session_id": "session-a"}},
            {"state": {"offset": 15, "session_id": "session-b"}},
            {"state": {"offset": 15, "rank_version": "version-a"}},
            {"state": {"offset": 15, "filter_ids": ["previous-item"]}},
        ):
            result = await _fetch_rank_feed_page(client, **{**base, **change})
            self.assertNotEqual(result["items"][0]["series_id"], "page-1")
        cached = await _fetch_rank_feed_page(client, **base)

        self.assertEqual(calls, 10)
        self.assertEqual(cached["items"][0]["series_id"], "page-1")
        self.assertEqual(cached["items"][0]["title"], "")

    async def test_rank_cache_refreshes_after_one_minute(self):
        calls = 0
        clock = [100.0]

        async def handler(**_call):
            nonlocal calls
            calls += 1
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": f"rank-{calls}"}]},
                "next_offset": 1, "has_more": False,
            }}}

        client = RecordingClient(handler)
        with patch("core.cache.time", SimpleNamespace(monotonic=lambda: clock[0])):
            observed = []
            for timestamp in (100.0, 159.0, 161.0):
                clock[0] = timestamp
                response = response_json(await duanju_rank(
                    request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
                ))
                observed.append(response["data"]["items"][0]["series_id"])

        self.assertEqual(observed, ["rank-1", "rank-1", "rank-2"])
        self.assertEqual(calls, 2)

    async def test_rank_failed_request_can_retry_without_a_cached_error(self):
        calls = 0

        async def handler(**_call):
            nonlocal calls
            calls += 1
            if calls == 1:
                return {"ok": False, "msg": "temporarily unavailable"}
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": "recovered"}]},
                "next_offset": 1, "has_more": False,
            }}}

        client = RecordingClient(handler)
        with self.assertLogs("fanqie.duanju", level="WARNING"):
            failed = await duanju_rank(
                request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
            )
        recovered = response_json(await duanju_rank(
            request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
        ))

        self.assertEqual(failed.status_code, 502)
        self.assertEqual(recovered["data"]["items"][0]["series_id"], "recovered")
        self.assertEqual(calls, 2)

    async def test_rank_non_advancing_page_does_not_prevent_an_immediate_retry(self):
        calls = 0

        async def handler(**_call):
            nonlocal calls
            calls += 1
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": "recovered"}]},
                "next_offset": 0 if calls == 1 else 1, "has_more": calls == 1,
            }}}

        client = RecordingClient(handler)
        with self.assertLogs("fanqie.duanju", level="WARNING"):
            failed = await duanju_rank(
                request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
            )
        recovered = response_json(await duanju_rank(
            request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
        ))

        self.assertEqual(failed.status_code, 502)
        self.assertEqual(recovered["code"], 0)
        self.assertEqual(recovered["data"]["items"][0]["series_id"], "recovered")
        self.assertEqual(calls, 2)

    async def test_cancelling_one_rank_request_preserves_the_shared_request(self):
        entered = asyncio.Event()
        resume = asyncio.Event()

        async def handler(**_call):
            entered.set()
            await resume.wait()
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": "shared-survivor"}]},
                "next_offset": 1, "has_more": False,
            }}}

        client = RecordingClient(handler)
        tasks = [asyncio.create_task(duanju_rank(
            request_for(client), board="ranklist_hot_sc", release_type="all", cursor="", limit=20,
        )) for _ in range(2)]
        await entered.wait()
        tasks[0].cancel()
        with self.assertRaises(asyncio.CancelledError):
            await tasks[0]
        resume.set()
        result = response_json(await tasks[1])

        self.assertEqual(result["data"]["items"][0]["series_id"], "shared-survivor")
        self.assertEqual(len(client.calls), 1)

    async def test_rank_coalesces_concurrent_requests_for_the_same_page(self):
        async def handler(**_call):
            await asyncio.sleep(0)
            return {"ok": True, "upstream": {"data": {
                "cell_view": {"video_data": [{"series_id": "shared-rank"}]},
                "next_offset": 1, "has_more": False,
            }}}

        client = RecordingClient(handler)
        responses = await asyncio.gather(*(
            duanju_rank(request_for(client), board="ranklist_followed", release_type="all", cursor="", limit=20)
            for _ in range(3)
        ))

        self.assertTrue(all(response_json(response)["code"] == 0 for response in responses))
        self.assertEqual(len(client.calls), 1)


if __name__ == "__main__":
    unittest.main()
