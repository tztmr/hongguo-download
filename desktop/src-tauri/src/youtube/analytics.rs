//! Read-only channel, video and audience Analytics reports.
use super::config::SecretString;
use crate::AppError;
use chrono::{Duration as Days, NaiveDate, SecondsFormat, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

const DATA_API: &str = "https://www.googleapis.com/youtube/v3";
const ANALYTICS_API: &str = "https://youtubeanalytics.googleapis.com/v2";
const PACIFIC_TIMEZONE: &str = "America/Los_Angeles";
const PLAYBACK_METRICS: &str =
    "views,engagedViews,estimatedMinutesWatched,averageViewDuration,averageViewPercentage";
const ACTIVITY_METRICS: &str = "views,engagedViews,estimatedMinutesWatched,averageViewDuration,averageViewPercentage,likes,comments,shares,subscribersGained,subscribersLost";
const SOURCE_METRICS: &str = "views,engagedViews,estimatedMinutesWatched";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAnalyticsSnapshot {
    pub channel_id: String,
    pub view_count: String,
    pub subscriber_count: Option<String>,
    pub hidden_subscriber_count: bool,
    pub video_count: Option<String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsMetrics {
    pub views: Option<f64>,
    pub engaged_views: Option<f64>,
    pub estimated_minutes_watched: Option<f64>,
    pub average_view_duration: Option<f64>,
    pub average_view_percentage: Option<f64>,
    pub likes: Option<f64>,
    pub comments: Option<f64>,
    pub shares: Option<f64>,
    pub subscribers_gained: Option<f64>,
    pub subscribers_lost: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRow {
    pub date: String,
    #[serde(flatten)]
    pub metrics: AnalyticsMetrics,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsComparison {
    pub start_date: String,
    pub end_date: String,
    #[serde(flatten)]
    pub metrics: AnalyticsMetrics,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsWarning {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsReport {
    pub channel_id: String,
    pub video_id: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub returned_end_date: Option<String>,
    #[serde(flatten)]
    pub metrics: AnalyticsMetrics,
    pub comparison: Option<AnalyticsComparison>,
    pub warnings: Vec<AnalyticsWarning>,
    pub rows: Vec<AnalyticsRow>,
    pub fetched_at: String,
    pub timezone: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BreakdownKind {
    Videos,
    ContentType,
    Traffic,
    Country,
    Device,
    Subscribed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub content_type: Option<String>,
    #[serde(flatten)]
    pub metrics: AnalyticsMetrics,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsBreakdown {
    pub channel_id: String,
    pub video_id: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub kind: BreakdownKind,
    pub rows: Vec<BreakdownRow>,
    pub truncated: bool,
    pub warnings: Vec<AnalyticsWarning>,
    pub fetched_at: String,
}

pub struct AnalyticsApi {
    client: Client,
    token: SecretString,
    channel_id: String,
    data_base: String,
    analytics_base: String,
}
fn error(code: &str, message: &str) -> AppError {
    AppError::new(code, message)
}
fn response_error() -> AppError {
    error(
        "YOUTUBE_ANALYTICS_RESPONSE",
        "YouTube 统计返回的数据不完整，请刷新重试",
    )
}
fn field(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn items(value: &Value) -> Option<&Vec<Value>> {
    value.get("items").and_then(Value::as_array)
}
fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
}
fn headers(value: &Value) -> Vec<String> {
    value["columnHeaders"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|header| header["name"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}
fn cell<'a>(row: &'a [Value], names: &[String], name: &str) -> Option<&'a Value> {
    names
        .iter()
        .position(|header| header == name)
        .and_then(|index| row.get(index))
}
fn metric(row: &[Value], names: &[String], name: &str) -> Option<f64> {
    cell(row, names, name).and_then(number)
}
fn metrics(row: &[Value], names: &[String]) -> AnalyticsMetrics {
    AnalyticsMetrics {
        views: metric(row, names, "views"),
        engaged_views: metric(row, names, "engagedViews"),
        estimated_minutes_watched: metric(row, names, "estimatedMinutesWatched"),
        average_view_duration: metric(row, names, "averageViewDuration"),
        average_view_percentage: metric(row, names, "averageViewPercentage"),
        likes: metric(row, names, "likes"),
        comments: metric(row, names, "comments"),
        shares: metric(row, names, "shares"),
        subscribers_gained: metric(row, names, "subscribersGained"),
        subscribers_lost: metric(row, names, "subscribersLost"),
    }
}
fn report_rows<'a>(
    value: &'a Value,
    dimension: Option<&str>,
) -> Result<Vec<&'a Vec<Value>>, AppError> {
    let names = headers(value);
    if !names.iter().any(|name| name == "views")
        || dimension.is_some_and(|name| !names.iter().any(|header| header == name))
    {
        return Err(response_error());
    }
    match value.get("rows") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(rows)) => rows
            .iter()
            .map(|row| row.as_array().ok_or_else(response_error))
            .collect(),
        _ => Err(response_error()),
    }
}
fn summary(value: &Value) -> Result<AnalyticsMetrics, AppError> {
    let rows = report_rows(value, None)?;
    if rows.len() > 1 {
        return Err(response_error());
    }
    Ok(rows
        .first()
        .map(|row| metrics(row, &headers(value)))
        .unwrap_or_default())
}
fn video_filter(video_id: Option<&str>) -> Result<Option<String>, AppError> {
    if let Some(id) = video_id {
        if id.len() != 11
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(error(
                "YOUTUBE_ANALYTICS_INVALID_VIDEO",
                "无效的视频 ID，请重新选择视频",
            ));
        }
        return Ok(Some(format!("video=={id}")));
    }
    Ok(None)
}
fn previous_period(start: &str, end: &str) -> Result<(String, String), AppError> {
    let start = NaiveDate::parse_from_str(start, "%Y-%m-%d").map_err(|_| response_error())?;
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d").map_err(|_| response_error())?;
    let days = (end - start).num_days() + 1;
    let previous_start = start
        .checked_sub_signed(Days::days(days))
        .ok_or_else(response_error)?;
    let previous_end = start
        .checked_sub_signed(Days::days(1))
        .ok_or_else(response_error)?;
    Ok((previous_start.to_string(), previous_end.to_string()))
}
fn warning(error: AppError) -> AnalyticsWarning {
    AnalyticsWarning {
        code: error.code,
        message: error.message,
    }
}
fn breakdown_spec(kind: BreakdownKind) -> (&'static str, &'static str) {
    match kind {
        BreakdownKind::Videos => ("video,creatorContentType", ACTIVITY_METRICS),
        BreakdownKind::ContentType => ("creatorContentType", PLAYBACK_METRICS),
        BreakdownKind::Country => ("country", PLAYBACK_METRICS),
        BreakdownKind::Subscribed => ("subscribedStatus", PLAYBACK_METRICS),
        // Traffic and device reports do not support average duration or engagement.
        BreakdownKind::Traffic => ("insightTrafficSourceType", SOURCE_METRICS),
        BreakdownKind::Device => ("deviceType", SOURCE_METRICS),
    }
}
fn classify_api_error(status: u16, reason: &str) -> (&'static str, &'static str) {
    match (status, reason) {
        (401, _) | (_, "insufficientPermissions") => (
            "YOUTUBE_ANALYTICS_AUTH_REQUIRED",
            "需要统计权限，请在设置中重新授权 YouTube 频道",
        ),
        (_, "quotaExceeded" | "dailyLimitExceeded") => (
            "YOUTUBE_QUOTA_EXCEEDED",
            "YouTube 接口配额已用完，请稍后重试",
        ),
        (403, "accessNotConfigured" | "serviceDisabled") => (
            "YOUTUBE_ANALYTICS_API_NOT_ENABLED",
            "请在凭证所属的 Google Cloud 项目启用 YouTube Analytics API，然后刷新数据（无需再次授权）",
        ),
        (403, "forbidden") => (
            "YOUTUBE_ANALYTICS_CHANNEL_FORBIDDEN",
            "当前 Google 账号无法读取该频道的 Analytics，请确认授权的是频道所有者账号",
        ),
        (403, _) => (
            "YOUTUBE_ANALYTICS_FORBIDDEN",
            "YouTube 拒绝了统计请求，请检查频道权限和 API 配置",
        ),
        (400, _) => ("YOUTUBE_ANALYTICS_UNSUPPORTED_REPORT", "YouTube 暂不支持此统计组合，请调整日期或在 Studio 查看该报表"),
        _ => (
            "YOUTUBE_ANALYTICS_FAILED",
            "读取 YouTube 统计失败，请稍后刷新重试",
        ),
    }
}

fn validate_dates(start_date: &str, end_date: &str) -> Result<(), AppError> {
    let start = NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .map_err(|_| error("YOUTUBE_ANALYTICS_INVALID_DATE", "统计日期格式无效"))?;
    let end = NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
        .map_err(|_| error("YOUTUBE_ANALYTICS_INVALID_DATE", "统计日期格式无效"))?;
    if start > end {
        return Err(error(
            "YOUTUBE_ANALYTICS_INVALID_DATE",
            "统计开始日期不能晚于结束日期",
        ));
    }
    if (end - start).num_days() > 366 {
        return Err(error(
            "YOUTUBE_ANALYTICS_INVALID_DATE",
            "单次统计最多查询 367 天",
        ));
    }
    Ok(())
}

impl AnalyticsApi {
    pub fn new(channel_id: &str, token: SecretString) -> Result<Self, AppError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|_| error("YOUTUBE_ANALYTICS_RESPONSE", "无法初始化 YouTube 统计请求"))?;
        Ok(Self {
            client,
            token,
            channel_id: channel_id.to_owned(),
            data_base: DATA_API.into(),
            analytics_base: ANALYTICS_API.into(),
        })
    }
    async fn request(&self, url: &str, params: &[(&str, &str)]) -> Result<Value, AppError> {
        let response = self
            .client
            .get(url)
            .query(params)
            .bearer_auth(self.token.expose_secret())
            .send()
            .await
            .map_err(|_| {
                error(
                    "YOUTUBE_ANALYTICS_NETWORK",
                    "连接 YouTube 统计服务失败，请刷新重试",
                )
            })?;
        let status = response.status();
        let data: Value = response.json().await.map_err(|_| response_error())?;
        if status.is_success() {
            return Ok(data);
        }
        let (code, message) =
            classify_api_error(status.as_u16(), &field(&data, "/error/errors/0/reason"));
        Err(error(code, message))
    }
    pub async fn snapshot(&self) -> Result<ChannelAnalyticsSnapshot, AppError> {
        let data = self
            .request(
                &format!("{}/channels", self.data_base),
                &[("part", "statistics"), ("id", &self.channel_id)],
            )
            .await?;
        let item = items(&data)
            .and_then(|rows| {
                rows.iter()
                    .find(|item| field(item, "/id") == self.channel_id)
            })
            .ok_or_else(|| error("YOUTUBE_CHANNEL_MISMATCH", "授权频道与所选频道不一致"))?;
        let count = |name: &str| {
            item["statistics"][name]
                .as_u64()
                .map(|v| v.to_string())
                .or_else(|| {
                    item["statistics"][name]
                        .as_str()
                        .filter(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
                        .map(str::to_owned)
                })
        };
        let hidden = item["statistics"]["hiddenSubscriberCount"]
            .as_bool()
            .unwrap_or(false);
        Ok(ChannelAnalyticsSnapshot {
            channel_id: self.channel_id.clone(),
            view_count: count("viewCount").ok_or_else(response_error)?,
            subscriber_count: if hidden {
                None
            } else {
                count("subscriberCount")
            },
            hidden_subscriber_count: hidden,
            video_count: count("videoCount"),
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        })
    }
    async fn query(
        &self,
        start: &str,
        end: &str,
        dimension: Option<&str>,
        metrics: &str,
        filter: Option<&str>,
        start_index: usize,
    ) -> Result<Value, AppError> {
        let mut params = vec![
            ("ids", format!("channel=={}", self.channel_id)),
            ("startDate", start.to_owned()),
            ("endDate", end.to_owned()),
            ("metrics", metrics.to_owned()),
            (
                "maxResults",
                if dimension == Some("day") {
                    "367"
                } else {
                    "200"
                }
                .into(),
            ),
        ];
        if let Some(dimension) = dimension {
            params.push(("dimensions", dimension.into()));
            params.push((
                "sort",
                if dimension == "day" { "day" } else { "-views" }.into(),
            ));
        }
        if let Some(filter) = filter {
            params.push(("filters", filter.into()));
        }
        if start_index > 1 {
            params.push(("startIndex", start_index.to_string()));
        }
        self.request(
            &format!("{}/reports", self.analytics_base),
            &params
                .iter()
                .map(|(k, v)| (*k, v.as_str()))
                .collect::<Vec<_>>(),
        )
        .await
    }
    pub async fn report_range(
        &self,
        start_date: &str,
        end_date: &str,
        video_id: Option<&str>,
    ) -> Result<AnalyticsReport, AppError> {
        validate_dates(start_date, end_date)?;
        let filter = video_filter(video_id)?;
        let daily = self
            .query(
                start_date,
                end_date,
                Some("day"),
                ACTIVITY_METRICS,
                filter.as_deref(),
                1,
            )
            .await?;
        let names = headers(&daily);
        let mut rows = Vec::new();
        let mut dates = HashSet::new();
        for row in report_rows(&daily, Some("day"))? {
            let date = cell(row, &names, "day")
                .and_then(Value::as_str)
                .ok_or_else(response_error)?;
            if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
                || date < start_date
                || date > end_date
                || !dates.insert(date.to_owned())
            {
                return Err(response_error());
            }
            rows.push(AnalyticsRow {
                date: date.to_owned(),
                metrics: metrics(row, &names),
            });
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date));
        let returned_end_date = rows.last().map(|row| row.date.clone());
        let mut report = AnalyticsReport {
            channel_id: self.channel_id.clone(),
            video_id: video_id.map(str::to_owned),
            start_date: start_date.into(),
            end_date: end_date.into(),
            returned_end_date: returned_end_date.clone(),
            metrics: AnalyticsMetrics::default(),
            comparison: None,
            warnings: vec![],
            rows,
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            timezone: PACIFIC_TIMEZONE.into(),
        };
        // Compare equal periods ending at the latest returned day. Future/unreturned
        // days do not become fabricated zeroes or depress the period comparison.
        if let Some(end) = returned_end_date {
            report.metrics = summary(
                &self
                    .query(
                        start_date,
                        &end,
                        None,
                        ACTIVITY_METRICS,
                        filter.as_deref(),
                        1,
                    )
                    .await?,
            )?;
            let (previous_start, previous_end) = previous_period(start_date, &end)?;
            match self
                .query(
                    &previous_start,
                    &previous_end,
                    None,
                    ACTIVITY_METRICS,
                    filter.as_deref(),
                    1,
                )
                .await
                .and_then(|value| summary(&value))
            {
                Ok(metrics) => {
                    report.comparison = Some(AnalyticsComparison {
                        start_date: previous_start,
                        end_date: previous_end,
                        metrics,
                    })
                }
                Err(error) => report.warnings.push(warning(error)),
            }
        }
        Ok(report)
    }
    pub async fn breakdown(
        &self,
        start_date: &str,
        end_date: &str,
        kind: BreakdownKind,
        video_id: Option<&str>,
    ) -> Result<AnalyticsBreakdown, AppError> {
        validate_dates(start_date, end_date)?;
        let filter = video_filter(video_id)?;
        if kind == BreakdownKind::Videos && video_id.is_some() {
            return Err(error(
                "YOUTUBE_ANALYTICS_INVALID_VIDEO",
                "热门视频榜单请在频道总览中查看",
            ));
        }
        let (dimension, metric_names) = breakdown_spec(kind);
        let key_name = dimension.split(',').next().unwrap();
        let mut result = AnalyticsBreakdown {
            channel_id: self.channel_id.clone(),
            video_id: video_id.map(str::to_owned),
            start_date: start_date.into(),
            end_date: end_date.into(),
            kind,
            rows: vec![],
            truncated: false,
            warnings: vec![],
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        let mut seen = HashSet::new();
        for page in 0..5 {
            let data = self
                .query(
                    start_date,
                    end_date,
                    Some(dimension),
                    metric_names,
                    filter.as_deref(),
                    page * 200 + 1,
                )
                .await?;
            let names = headers(&data);
            let rows = report_rows(&data, Some(key_name))?;
            let count = rows.len();
            if count > 200 {
                return Err(response_error());
            }
            for row in rows {
                let key = cell(row, &names, key_name)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(response_error)?
                    .to_owned();
                if kind == BreakdownKind::Videos && video_filter(Some(&key)).is_err() {
                    return Err(response_error());
                }
                let content_type = cell(row, &names, "creatorContentType")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if !seen.insert((key.clone(), content_type.clone())) {
                    return Err(response_error());
                }
                result.rows.push(BreakdownRow {
                    key,
                    content_type,
                    title: None,
                    thumbnail_url: None,
                    metrics: metrics(row, &names),
                });
            }
            if count < 200 {
                break;
            }
            if kind == BreakdownKind::Videos || page == 4 {
                result.truncated = true;
                break;
            }
        }
        if kind == BreakdownKind::Videos {
            self.video_titles(&mut result).await;
        }
        Ok(result)
    }
    async fn video_titles(&self, result: &mut AnalyticsBreakdown) {
        for rows in result.rows.chunks_mut(50) {
            let ids = rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>()
                .join(",");
            match self
                .request(
                    &format!("{}/videos", self.data_base),
                    &[("part", "snippet"), ("id", &ids)],
                )
                .await
            {
                Ok(data) => {
                    if let Some(items) = items(&data) {
                        for row in rows {
                            if let Some(item) = items.iter().find(|item| {
                                field(item, "/id") == row.key
                                    && field(item, "/snippet/channelId") == self.channel_id
                            }) {
                                row.title = Some(field(item, "/snippet/title"));
                                row.thumbnail_url =
                                    Some(field(item, "/snippet/thumbnails/medium/url"));
                            }
                        }
                    }
                }
                Err(error) => {
                    result.warnings.push(warning(error));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "analytics_tests.rs"]
mod tests;
