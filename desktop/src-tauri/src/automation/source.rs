//! Pure adapters for the local duanju API. Unknown completion stays unknown.
use crate::app_error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub book_id: String,
    pub title: String,
    pub cover: String,
    pub summary: String,
    pub category: String,
    pub tags: Vec<String>,
    pub episode_count: u64,
    pub online_time: Option<i64>,
    pub content_type: i64,
    pub release_type: String,
    pub complete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub index: u32,
    pub item_id: String,
    pub title: String,
}

fn string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

pub fn parse_candidate(value: &Value) -> Option<Candidate> {
    let book_id = ["book_id", "series_id"]
        .iter()
        .map(|key| string(&value[key]))
        .find(|id| !id.trim().is_empty())?;
    let title = value["title"].as_str()?.to_owned();
    if title.trim().is_empty() {
        return None;
    }
    let content_type = integer(&value["content_type"]).unwrap_or(1);
    let release_type = value["release_type"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if value["video_category_type"].as_str() == Some("ai_video") {
                "ai_playlet"
            } else if matches!(content_type, 2 | 1004) {
                "comic_series_rank"
            } else if content_type == 1 {
                "playlet"
            } else {
                ""
            }
        })
        .to_owned();
    Some(Candidate {
        book_id,
        title,
        cover: string(&value["cover"]),
        summary: string(&value["abstract"]),
        category: string(&value["category"]),
        tags: value["category_tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        episode_count: integer(&value["episode_count"]).unwrap_or(0).max(0) as u64,
        online_time: integer(&value["online_time"]).filter(|time| *time > 0),
        content_type,
        release_type,
        complete: completion_evidence(value),
    })
}

pub fn eligible(candidate: &Candidate, config: &Value, now: i64) -> bool {
    let label = match candidate.release_type.as_str() {
        "playlet" => "真人剧",
        "comic_series_rank" => "漫剧",
        "ai_playlet" => "AI剧",
        _ => return false,
    };
    if let Some(types) = config["types"].as_array() {
        if !types.iter().any(|v| v.as_str() == Some(label)) {
            return false;
        }
    }
    if config["scope"].as_str() == Some("today") {
        // Shanghai is UTC+8 without daylight saving for contemporary releases.
        let shanghai_day = |time: i64| time.saturating_add(8 * 3600).div_euclid(86400);
        if candidate.online_time.map(shanghai_day) != Some(shanghai_day(now)) {
            return false;
        }
    }
    let searchable = format!(
        "{} {} {} {}",
        candidate.title,
        candidate.summary,
        candidate.category,
        candidate.tags.join(" ")
    )
    .to_lowercase();
    let keywords = |key: &str| -> Vec<String> {
        config[key]
            .as_str()
            .unwrap_or("")
            .split([',', '，'])
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let include = keywords("keywords");
    (include.is_empty() || include.iter().any(|word| searchable.contains(word)))
        && !keywords("exclude")
            .iter()
            .any(|word| searchable.contains(word))
}

pub fn feed_path(release_type: &str, scope: &str, cursor: &str) -> String {
    let endpoint = if scope == "today" {
        "/api/duanju/new-releases?"
    } else {
        "/api/duanju/rank?board=ranklist_new_rank_sc&"
    };
    format!(
        "{endpoint}type={}&limit=20&cursor={}",
        urlencoding::encode(release_type),
        urlencoding::encode(cursor)
    )
}

pub fn parse_catalogue(value: &Value, expected: u64) -> Result<Vec<Episode>, AppError> {
    let invalid = |message: &str| AppError::new("AUTOMATION_CATALOGUE_INVALID", message);
    let mut body = value;
    for _ in 0..4 {
        if body.get("items").is_some() {
            break;
        }
        if let Some(data) = body.get("data") {
            body = data;
        } else {
            break;
        }
    }
    let rows = body["items"]
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or_else(|| invalid("剧集目录为空，等待源目录恢复后重试"))?;
    if expected > 0 && rows.len() as u64 != expected {
        return Err(invalid("剧集目录数量与源集数不一致，暂停以避免缺集"));
    }
    let mut ids = HashSet::new();
    let mut episodes = Vec::with_capacity(rows.len());
    for row in rows {
        let item_id = string(&row["item_id"]);
        if item_id.trim().is_empty() || !ids.insert(item_id.clone()) {
            return Err(invalid("剧集目录包含空 ID 或重复 ID，暂停以避免错误合并"));
        }
        let index = integer(&row["index"])
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| invalid("剧集目录包含无效集数"))?;
        episodes.push(Episode {
            index,
            item_id,
            title: row["title"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("第{index}集")),
        });
    }
    episodes.sort_by_key(|episode| episode.index);
    if episodes
        .iter()
        .enumerate()
        .any(|(offset, episode)| u64::from(episode.index) != offset as u64 + 1)
    {
        return Err(invalid("剧集序号不连续或重复，暂停以避免缺集"));
    }
    Ok(episodes)
}

/// Current source totals, independently declared by series metadata. The local
/// detail endpoint exposes `data.chapter_number`; catalog exposes the same
/// field under `raw.data.book_info`. Never turn returned row count into a total,
/// since that would hide truncated catalogs. Conflicting declarations stay unknown.
pub fn episode_count_evidence(value: &Value) -> Option<u64> {
    fn inspect(value: &Value, depth: u8, counts: &mut HashSet<u64>) {
        if depth > 8 {
            return;
        }
        let Some(object) = value.as_object() else {
            return;
        };
        for key in ["chapter_number", "episode_count", "episode_cnt"] {
            if let Some(count) = object.get(key).and_then(integer).filter(|n| *n > 0) {
                counts.insert(count as u64);
            }
        }
        for key in [
            "data",
            "raw",
            "book_info",
            "book",
            "video_detail",
            "series_info",
            "detail",
        ] {
            if let Some(child) = object.get(key) {
                inspect(child, depth + 1, counts);
            }
        }
    }
    let mut counts = HashSet::new();
    inspect(value, 0, &mut counts);
    (counts.len() == 1)
        .then(|| counts.into_iter().next())
        .flatten()
}

/// Look only at status fields and known series metadata containers. Never scan
/// titles, synopsis, arbitrary descendant objects or undocumented numeric enums.
pub fn completion_evidence(value: &Value) -> Option<bool> {
    fn status(text: &str) -> Option<bool> {
        let mut complete = false;
        for part in text.split(['·', '|', ',', '，', '、']) {
            let part = part.trim();
            if matches!(part, "连载中" | "未完结" | "更新中") {
                return Some(false);
            }
            if matches!(part, "完结" | "已完结") {
                complete = true;
            }
            if let Some(number) = part.strip_prefix('全').and_then(|s| s.strip_suffix('集')) {
                if number.trim().parse::<u64>().is_ok_and(|n| n > 0) {
                    complete = true;
                }
            }
        }
        complete.then_some(true)
    }
    fn inspect(value: &Value, depth: u8, found: &mut Option<bool>) {
        if depth > 8 || *found == Some(false) {
            return;
        }
        let Some(object) = value.as_object() else {
            return;
        };
        let mut add = |evidence: Option<bool>| {
            if evidence == Some(false) || (found.is_none() && evidence == Some(true)) {
                *found = evidence;
            }
        };
        for key in [
            "complete",
            "completed",
            "is_complete",
            "is_completed",
            "is_finished",
        ] {
            add(object.get(key).and_then(Value::as_bool));
        }
        for key in [
            "sub_title",
            "serial_status",
            "status_text",
            "completion_status",
        ] {
            add(object.get(key).and_then(Value::as_str).and_then(status));
        }
        for key in ["sub_title_list", "secondary_info_list"] {
            for tag in object
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                add(tag.as_str().and_then(status));
                add(tag.get("content").and_then(Value::as_str).and_then(status));
            }
        }
        for key in [
            "data",
            "raw",
            "book_info",
            "book",
            "video_detail",
            "series_info",
            "detail",
        ] {
            if let Some(child) = object.get(key) {
                inspect(child, depth + 1, found);
            }
        }
    }
    let mut found = None;
    inspect(value, 0, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate() -> Candidate {
        parse_candidate(
            &json!({"book_id":"42","title":"归途 第二季 S02", "content_type":1,
            "episode_count":80,"abstract":"都市重生", "online_time": 86400 - 8 * 3600}),
        )
        .unwrap()
    }

    #[test]
    fn preserves_season_and_does_not_infer_completion_from_total() {
        let c = candidate();
        assert_eq!(c.title, "归途 第二季 S02");
        assert_eq!(c.complete, None);
        assert_eq!(
            completion_evidence(&json!({"serial_status":2,"episode_count":80})),
            None
        );
    }

    #[test]
    fn date_scope_uses_shanghai_midnight_and_all_new_do_not_filter() {
        let c = candidate();
        assert!(eligible(&c, &json!({"scope":"today"}), 86400 - 8 * 3600));
        assert!(!eligible(
            &c,
            &json!({"scope":"today"}),
            86400 - 8 * 3600 - 1
        ));
        assert!(eligible(&c, &json!({"scope":"all"}), 1));
        assert!(eligible(&c, &json!({"scope":"new"}), 1));
        let mut c = c;
        c.online_time = None;
        assert!(!eligible(&c, &json!({"scope":"today"}), 86400));
        assert!(eligible(
            &c,
            &json!({"scope":"all", "completeOnly":true}),
            86400
        ));
    }

    #[test]
    fn keyword_and_type_filters() {
        let c = candidate();
        assert!(eligible(
            &c,
            &json!({"types":["真人剧"],"keywords":"仙侠， 重生"}),
            1
        ));
        assert!(!eligible(&c, &json!({"exclude":"测试, S02"}), 1));
        assert!(!eligible(&c, &json!({"types":["漫剧"]}), 1));
        assert!(!eligible(&c, &json!({"types":[]}), 1));
    }

    #[test]
    fn completion_only_uses_explicit_series_metadata() {
        assert_eq!(
            completion_evidence(
                &json!({"data":{"book_info":{"sub_title":"东方仙侠·全300集·演员甲"}}})
            ),
            Some(true)
        );
        assert_eq!(
            completion_evidence(&json!({"raw":{"data":{"completed":true}}})),
            Some(true)
        );
        assert_eq!(
            completion_evidence(&json!({"sub_title_list":[{"content":"已完结"}]})),
            Some(true)
        );
        assert_eq!(
            completion_evidence(&json!({"sub_title":"连载中","completed":true})),
            Some(false)
        );
        assert_eq!(
            completion_evidence(
                &json!({"title":"已完结", "abstract":"全80集", "data":{"series_intro":"已完结"}, "actor":{"completed":true}})
            ),
            None
        );
        assert_eq!(
            completion_evidence(&json!({"sub_title":"更新至80集", "episode_count":80})),
            None
        );
    }

    #[test]
    fn preserved_live_source_status_marks_the_candidate_complete() {
        let candidate = parse_candidate(&json!({
            "book_id":"7683098724637101080", "title":"婚后恋爱禁止条例", "episode_count":64,
            "sub_title":"都市日常·都市脑洞·全64集", "sub_title_list":[{"content":"全64集"}]
        }))
        .unwrap();
        assert_eq!(candidate.complete, Some(true));
        assert_eq!(candidate.episode_count, 64);
    }

    #[test]
    fn catalogue_rejects_duplicates_gaps_and_count_mismatch() {
        let rows = json!({"data":{"items":[{"index":2,"item_id":"b"},{"index":1,"item_id":"a"}]}});
        let episodes = parse_catalogue(&rows, 2).unwrap();
        assert_eq!(episodes[0].item_id, "a");
        assert!(parse_catalogue(&rows, 3).is_err());
        assert!(parse_catalogue(
            &json!({"items":[{"index":1,"item_id":"a"},{"index":2,"item_id":"a"}]}),
            0
        )
        .is_err());
        assert!(parse_catalogue(
            &json!({"items":[{"index":1,"item_id":"a"},{"index":3,"item_id":"b"}]}),
            0
        )
        .is_err());
        assert!(parse_catalogue(
            &json!({"items":[{"index":1,"item_id":"a"},{"index":1,"item_id":"b"}]}),
            0
        )
        .is_err());
        assert!(parse_catalogue(&json!({"items":[]}), 0).is_err());
    }

    #[test]
    fn feed_routes_and_cursor_are_correct() {
        assert_eq!(
            feed_path("playlet", "today", "a&b"),
            "/api/duanju/new-releases?type=playlet&limit=20&cursor=a%26b"
        );
        assert_eq!(
            feed_path("comic_series_rank", "all", ""),
            "/api/duanju/rank?board=ranklist_new_rank_sc&type=comic_series_rank&limit=20&cursor="
        );
        assert_eq!(
            feed_path("ai_playlet", "new", ""),
            "/api/duanju/rank?board=ranklist_new_rank_sc&type=ai_playlet&limit=20&cursor="
        );
    }
    #[test]
    fn current_detail_and_catalog_totals_can_replace_an_observed_old_total() {
        // Shapes verified against the local API for 7683098724637101080.
        assert_eq!(
            episode_count_evidence(
                &json!({"data":{"data":{"chapter_number":"64","serial_count":"64"}}})
            ),
            Some(64)
        );
        assert_eq!(
            episode_count_evidence(
                &json!({"data":{"raw":{"data":{"book_info":{"chapter_number":"64"}}}}})
            ),
            Some(64)
        );
        let catalog = json!({"items":[{"index":1,"item_id":"a"},{"index":2,"item_id":"b"}],"raw":{"data":{"book_info":{"chapter_number":"2"}}}});
        assert!(parse_catalogue(&catalog, 1).is_err());
        assert_eq!(
            parse_catalogue(&catalog, episode_count_evidence(&catalog).unwrap())
                .unwrap()
                .len(),
            2
        );
        assert_eq!(completion_evidence(&catalog), None);
    }
    #[test]
    fn totals_do_not_use_estimates_row_counts_or_conflicting_metadata() {
        for value in [
            json!({"items":[{"index":1,"item_id":"a"}],"estimated_chapter_count":"100","serial_count":"1"}),
            json!({"chapter_number":"0"}),
            json!({"chapter_number":-1}),
            json!({"chapter_number":"unknown"}),
            json!({"chapter_number":"64","video_detail":{"episode_cnt":65}}),
            json!({"actor":{"episode_count":64},"abstract":"全64集"}),
        ] {
            assert_eq!(episode_count_evidence(&value), None);
        }
        let truncated = json!({"items":[{"index":1,"item_id":"a"}],"raw":{"data":{"book_info":{"chapter_number":"2"}}}});
        assert!(parse_catalogue(&truncated, episode_count_evidence(&truncated).unwrap()).is_err());
    }
}
