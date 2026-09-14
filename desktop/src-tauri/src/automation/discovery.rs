//! Fair, bounded source rotation. Each recommendation cursor owns one device.
use super::model::{fingerprint, number, text};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub signature: String,
    pub streams: Vec<Stream>,
    pub position: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub kind: String,
    pub label: String,
    pub release_type: String,
    pub argument: String,
    pub cursor: String,
    pub offset: u64,
    pub passback: String,
    pub page: u64,
    pub round: u64,
    pub failures: u64,
    pub ready_at: u64,
    pub done: bool,
    pub last_page: String,
}

pub fn enabled(config: &Value, key: &str) -> bool {
    config[key].as_bool().unwrap_or(true)
}

impl Discovery {
    pub fn prepare(&mut self, config: &Value) -> bool {
        let signature = fingerprint(&config.to_string());
        if self.signature == signature && !self.streams.is_empty() && self.more() {
            return false;
        }
        *self = Self {
            signature,
            ..Self::default()
        };
        let mut add = |kind: &str, label: String, release_type: &str, argument: &str| {
            self.streams.push(Stream {
                kind: kind.into(),
                label,
                release_type: release_type.into(),
                argument: argument.into(),
                ..Stream::default()
            });
        };
        if enabled(config, "collectRecommend") {
            add("recommend", "漫剧推荐".into(), "", "32");
            add("recommend", "找剧推荐".into(), "", "8");
        }
        let types: Vec<_> = config["types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| match v.as_str() {
                Some("漫剧") => Some(("comic_series_rank", "漫剧")),
                Some("AI剧") => Some(("ai_playlet", "AI剧")),
                _ => None,
            })
            .collect();
        for (release, label) in &types {
            if enabled(config, "collectNew") {
                add("new", format!("{label}新剧"), release, "");
            }
        }
        if enabled(config, "collectRank") {
            for (board, label) in [
                ("ranklist_hot_sc", "推荐榜"),
                ("ranklist_hot_play_sc", "热播榜"),
                ("ranklist_new_rank_sc", "新剧榜"),
                ("ranklist_hot_search_sc", "热搜榜"),
                ("ranklist_must_watch", "必看榜"),
                ("ranklist_followed", "收藏榜"),
                ("ranklist_prestige", "臻果榜"),
                ("ranklist_subscribe", "预约榜"),
            ] {
                for (release, kind) in &types {
                    add("rank", format!("{kind} · {label}"), release, board);
                }
            }
        }
        if enabled(config, "collectSearch") {
            let mut words = Vec::new();
            for word in text(config, "keywords")
                .split([',', '，'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if !words.contains(&word) {
                    words.push(word);
                }
            }
            for word in words.into_iter().take(10) {
                add("search", format!("关键词 · {word}"), "", word);
            }
        }
        true
    }
    pub fn more(&self) -> bool {
        self.streams.iter().any(|s| !s.done)
    }
    pub fn select(&mut self, now: u64) -> Option<usize> {
        for step in 0..self.streams.len() {
            let index = (self.position + step) % self.streams.len();
            let s = &self.streams[index];
            if !s.done && s.ready_at <= now {
                self.position = (index + 1) % self.streams.len();
                return Some(index);
            }
        }
        None
    }
    pub fn next_at(&self, now: u64, config: &Value) -> u64 {
        self.streams
            .iter()
            .filter(|s| !s.done)
            .map(|s| s.ready_at.max(now + 3))
            .min()
            .unwrap_or(now + number(config, "interval", 5) * 60)
    }
}

impl Stream {
    pub fn path(&self) -> String {
        let enc = urlencoding::encode;
        match self.kind.as_str() {
            "recommend" => format!(
                "/api/duanju/collection/recommendation?tab={}&cursor={}",
                self.argument,
                enc(&self.cursor)
            ),
            "new" => format!(
                "/api/duanju/new-releases?type={}&limit=20&cursor={}",
                self.release_type,
                enc(&self.cursor)
            ),
            "rank" => format!(
                "/api/duanju/rank?board={}&type={}&limit=20&cursor={}",
                self.argument,
                self.release_type,
                enc(&self.cursor)
            ),
            _ => format!(
                "/api/duanju/search?content_type=manju&key={}&offset={}&passback={}",
                enc(&self.argument),
                self.offset,
                enc(&self.passback)
            ),
        }
    }
    fn end_round(&mut self, config: &Value) {
        self.round += 1;
        self.done = self.kind != "recommend" || self.round >= number(config, "recommendDevices", 3);
        self.cursor.clear();
        self.offset = 0;
        self.passback.clear();
        self.page = 0;
        self.last_page.clear();
    }
    pub fn failed(&mut self, now: u64, config: &Value) {
        self.failures += 1;
        // A failed/expired cursor must not be retried forever. Other streams
        // continue while this one waits, with a fresh first-page request.
        self.cursor.clear();
        self.offset = 0;
        self.passback.clear();
        self.page = 0;
        self.last_page.clear();
        self.ready_at = now + 30 * (1 << self.failures.min(3));
        if self.failures >= 3 {
            self.end_round(config);
        }
    }
    pub fn accept(&mut self, page: &Value, config: &Value) -> String {
        self.failures = 0;
        self.ready_at = 0;
        self.page += 1;
        let ids: Vec<_> = page["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| {
                v["series_id"]
                    .as_str()
                    .or_else(|| v["book_id"].as_str())
                    .unwrap_or("")
            })
            .collect();
        let mark = fingerprint(&ids.join(","));
        let repeated = !ids.is_empty() && mark == self.last_page;
        self.last_page = mark;
        let more = page["has_more"].as_bool().unwrap_or(false);
        let cursor = text(page, "next_cursor").to_owned();
        let offset = page["next_offset"].as_u64().unwrap_or(0);
        let stalled = more
            && if self.kind == "search" {
                offset <= self.offset
            } else {
                cursor.is_empty() || cursor == self.cursor
            };
        let note = if repeated {
            "返回重复页，结束当前轮次".into()
        } else if stalled {
            "分页未前进，结束当前轮次".into()
        } else if self.page >= number(config, "collectPages", 10) {
            "已达每轮页数上限".into()
        } else {
            text(page, "reason").to_owned()
        };
        if !more || repeated || stalled || self.page >= number(config, "collectPages", 10) {
            self.end_round(config);
        } else {
            self.cursor = cursor;
            self.offset = offset;
            self.passback = text(page, "next_passback").into();
        }
        note
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> Value {
        json!({"types":["漫剧","AI剧"],"keywords":"重生，古代,重生"})
    }
    #[test]
    fn default_sources_cover_recommendations_all_boards_and_deduped_keywords() {
        let mut d = Discovery::default();
        d.prepare(&config());
        assert_eq!(
            d.streams.iter().filter(|s| s.kind == "recommend").count(),
            2
        );
        assert_eq!(d.streams.iter().filter(|s| s.kind == "rank").count(), 16);
        assert_eq!(d.streams.iter().filter(|s| s.kind == "search").count(), 2);
        assert!(d
            .streams
            .last()
            .unwrap()
            .path()
            .contains("%E5%8F%A4%E4%BB%A3"));
    }
    #[test]
    fn failing_recommendation_does_not_block_other_sources() {
        let mut d = Discovery::default();
        d.prepare(&config());
        assert_eq!(d.select(100), Some(0));
        d.streams[0].failed(100, &config());
        assert_eq!(d.select(100), Some(1));
        assert!(d.streams[0].ready_at > 100);
        assert!(!d.prepare(&config()));
    }
    #[test]
    fn recommendation_ends_page_bound_and_rotates_device_without_reusing_cursor() {
        let c = json!({"collectPages":"1", "recommendDevices":"2"});
        let mut s = Stream {
            kind: "recommend".into(),
            argument: "32".into(),
            ..Stream::default()
        };
        let p = json!({"items":[{"series_id":"7680000000000000001"}],"has_more":true,"next_cursor":"opaque"});
        s.accept(&p, &c);
        assert_eq!(s.round, 1);
        assert!(!s.done);
        assert!(s.cursor.is_empty());
        s.accept(&p, &c);
        assert!(s.done);
    }
    #[test]
    fn search_carries_offset_and_passback_and_stops_on_stalled_page() {
        let mut s = Stream {
            kind: "search".into(),
            argument: "古代".into(),
            ..Stream::default()
        };
        let p = json!({"items":[{"series_id":"1"}],"has_more":true,"next_offset":10,"next_passback":"a+b/"});
        s.accept(&p, &config());
        assert!(s.path().contains("offset=10&passback=a%2Bb%2F"));
        s.accept(&p, &config());
        assert!(s.done);
    }
}
