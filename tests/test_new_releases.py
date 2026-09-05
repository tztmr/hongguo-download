import unittest
from datetime import datetime
from zoneinfo import ZoneInfo

from core.new_releases import (
    CursorStore,
    collect_today_releases,
    is_shanghai_today,
    normalize_metrics,
)


SHANGHAI = ZoneInfo("Asia/Shanghai")


class NewReleaseDomainTests(unittest.TestCase):
    def test_normalize_metrics_preserves_zero_and_missing(self):
        upstream = {
            "data": {
                "unrelated": {"series_id": "other", "create_time": 1},
                "series": {
                    "series_id": "s1",
                    "create_time": 1_788_321_600,
                    "series_play_cnt": 0,
                    "hot_score": 20,
                    "followed_cnt": None,
                    "digg_cnt": 0,
                },
            }
        }

        value = normalize_metrics(upstream, "s1")

        self.assertEqual(value["online_time"], 1_788_321_600)
        self.assertEqual(value["play_count"], 0)
        self.assertEqual(value["hot_count"], 20)
        self.assertIsNone(value["collect_count"])
        self.assertEqual(value["like_count"], 0)

    def test_normalize_metrics_returns_missing_values_for_unknown_series(self):
        value = normalize_metrics({"data": {"series": {"series_id": "other"}}}, "s1")

        self.assertEqual(
            value,
            {
                "online_time": None,
                "play_count": None,
                "hot_count": None,
                "collect_count": None,
                "like_count": None,
            },
        )

    def test_today_uses_shanghai_calendar_boundaries(self):
        now = datetime(2026, 9, 2, 15, 0, tzinfo=SHANGHAI)

        self.assertTrue(
            is_shanghai_today(
                int(datetime(2026, 9, 2, 0, 0, tzinfo=SHANGHAI).timestamp()), now
            )
        )
        self.assertTrue(
            is_shanghai_today(
                int(datetime(2026, 9, 2, 23, 59, 59, tzinfo=SHANGHAI).timestamp()),
                now,
            )
        )
        self.assertFalse(
            is_shanghai_today(
                int(datetime(2026, 9, 1, 23, 59, 59, tzinfo=SHANGHAI).timestamp()),
                now,
            )
        )
        self.assertFalse(is_shanghai_today(0, now))
        self.assertFalse(is_shanghai_today("bad", now))


class CursorAndCollectorTests(unittest.IsolatedAsyncioTestCase):
    def test_cursor_is_short_and_rejects_wrong_type_date_or_expiry(self):
        now = [1_000.0]
        store = CursorStore(ttl_seconds=600, max_entries=2, clock=lambda: now[0])
        token = store.put(
            "all",
            "2026-09-02",
            {"upstream": 18, "seen": ["s1"], "buffer": []},
        )

        self.assertLess(len(token), 200)
        self.assertEqual(store.get(token, "all", "2026-09-02")["upstream"], 18)
        for release_type, date in (
            ("playlet", "2026-09-02"),
            ("all", "2026-09-03"),
        ):
            with self.assertRaisesRegex(ValueError, "分页游标已失效"):
                store.get(token, release_type, date)

        now[0] = 1_601.0
        with self.assertRaisesRegex(ValueError, "分页游标已失效"):
            store.get(token, "all", "2026-09-02")

    def test_cursor_store_evicts_the_oldest_entry_at_capacity(self):
        now = [1_000.0]
        store = CursorStore(ttl_seconds=600, max_entries=2, clock=lambda: now[0])
        first = store.put("all", "2026-09-02", {"upstream": 1})
        now[0] += 1
        second = store.put("all", "2026-09-02", {"upstream": 2})
        now[0] += 1
        third = store.put("all", "2026-09-02", {"upstream": 3})

        with self.assertRaisesRegex(ValueError, "分页游标已失效"):
            store.get(first, "all", "2026-09-02")
        self.assertEqual(store.get(second, "all", "2026-09-02")["upstream"], 2)
        self.assertEqual(store.get(third, "all", "2026-09-02")["upstream"], 3)

    async def test_collects_twenty_unique_today_items_and_buffers_overfetch(self):
        today_noon = int(datetime(2026, 9, 2, 12, tzinfo=SHANGHAI).timestamp())
        pages = {
            None: {
                "items": [{"series_id": f"s{i}", "title": f"剧{i}"} for i in range(1, 19)],
                "next": 18,
                "has_more": True,
            },
            18: {
                "items": [{"series_id": f"s{i}", "title": f"剧{i}"} for i in range(19, 37)],
                "next": 36,
                "has_more": False,
            },
        }
        fetch_calls = []

        async def fetch_page(state):
            fetch_calls.append(state)
            return pages[state]

        async def fetch_metrics(item):
            index = int(item["series_id"][1:])
            return {
                "online_time": today_noon + index,
                "play_count": index,
                "hot_count": index * 2,
                "collect_count": index * 3,
                "like_count": index * 4,
            }

        store = CursorStore()
        first = await collect_today_releases(
            fetch_page=fetch_page,
            fetch_metrics=fetch_metrics,
            release_type="all",
            cursor="",
            limit=20,
            now=datetime(2026, 9, 2, 12, tzinfo=SHANGHAI),
            cursor_store=store,
        )

        self.assertEqual(fetch_calls, [None, 18])
        self.assertEqual(len(first["items"]), 20)
        self.assertEqual(len({item["series_id"] for item in first["items"]}), 20)
        self.assertTrue(first["has_more"])
        self.assertTrue(first["next_cursor"])
        self.assertEqual(first["items"][0]["series_id"], "s36")

        second = await collect_today_releases(
            fetch_page=fetch_page,
            fetch_metrics=fetch_metrics,
            release_type="all",
            cursor=first["next_cursor"],
            limit=20,
            now=datetime(2026, 9, 2, 12, tzinfo=SHANGHAI),
            cursor_store=store,
        )

        self.assertEqual(fetch_calls, [None, 18])
        self.assertEqual(len(second["items"]), 16)
        self.assertFalse(second["has_more"])
        self.assertEqual(second["next_cursor"], "")

    async def test_excludes_non_today_and_keeps_known_today_item_when_metrics_fail(self):
        today = int(datetime(2026, 9, 2, 9, tzinfo=SHANGHAI).timestamp())
        yesterday = int(datetime(2026, 9, 1, 23, tzinfo=SHANGHAI).timestamp())

        async def fetch_page(_state):
            return {
                "items": [
                    {"series_id": "today"},
                    {"series_id": "old"},
                    {"series_id": "broken", "online_time": today + 1},
                ],
                "next": 3,
                "has_more": False,
            }

        async def fetch_metrics(item):
            if item["series_id"] == "broken":
                raise RuntimeError("upstream unavailable")
            return {
                "online_time": today if item["series_id"] == "today" else yesterday,
                "play_count": 0,
                "hot_count": 0,
                "collect_count": 0,
                "like_count": 0,
            }

        result = await collect_today_releases(
            fetch_page=fetch_page,
            fetch_metrics=fetch_metrics,
            release_type="all",
            cursor="",
            limit=20,
            now=datetime(2026, 9, 2, 12, tzinfo=SHANGHAI),
            cursor_store=CursorStore(),
        )

        self.assertEqual(
            [item["series_id"] for item in result["items"]], ["broken", "today"]
        )
        broken = result["items"][0]
        self.assertIsNone(broken["play_count"])
        self.assertIsNone(broken["like_count"])
        self.assertFalse(result["has_more"])

    async def test_continues_past_ten_pages_until_it_finds_today_or_upstream_ends(self):
        yesterday = int(datetime(2026, 9, 1, 12, tzinfo=SHANGHAI).timestamp())
        today = int(datetime(2026, 9, 2, 12, tzinfo=SHANGHAI).timestamp())
        fetch_calls = 0

        async def fetch_page(state):
            nonlocal fetch_calls
            fetch_calls += 1
            return {
                "items": [{"series_id": f"item-{fetch_calls}"}],
                "next": fetch_calls,
                "has_more": fetch_calls < 13,
            }

        async def fetch_metrics(item):
            index = int(item["series_id"].split("-")[-1])
            return {
                "online_time": today if index == 13 else yesterday,
                "play_count": 1,
                "hot_count": 2,
                "collect_count": 3,
                "like_count": 4,
            }

        result = await collect_today_releases(
            fetch_page=fetch_page,
            fetch_metrics=fetch_metrics,
            release_type="all",
            cursor="",
            limit=20,
            now=datetime(2026, 9, 2, 12, tzinfo=SHANGHAI),
            cursor_store=CursorStore(),
        )

        self.assertEqual(fetch_calls, 13)
        self.assertEqual([item["series_id"] for item in result["items"]], ["item-13"])
        self.assertFalse(result["has_more"])
        self.assertEqual(result["next_cursor"], "")

    async def test_rejects_an_upstream_page_that_claims_more_without_progress(self):
        async def fetch_page(_state):
            return {"items": [], "next": None, "has_more": True}

        async def fetch_metrics(_item):
            raise AssertionError("empty page must not fetch metrics")

        with self.assertRaisesRegex(RuntimeError, "上游分页状态未前进"):
            await collect_today_releases(
                fetch_page=fetch_page,
                fetch_metrics=fetch_metrics,
                release_type="playlet",
                cursor="",
                limit=20,
                now=datetime(2026, 9, 2, 12, tzinfo=SHANGHAI),
                cursor_store=CursorStore(),
            )


if __name__ == "__main__":
    unittest.main()
