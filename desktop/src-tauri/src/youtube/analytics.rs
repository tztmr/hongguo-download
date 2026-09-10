//! Read-only YouTube channel statistics and daily Analytics reports.
use super::config::SecretString;
use crate::AppError;
use chrono::{NaiveDate, SecondsFormat, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const DATA_API: &str = "https://www.googleapis.com/youtube/v3";
const ANALYTICS_API: &str = "https://youtubeanalytics.googleapis.com/v2";
const PACIFIC_TIMEZONE: &str = "America/Los_Angeles";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAnalyticsSnapshot {
    pub channel_id: String,
    pub view_count: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRow {
    pub date: String,
    pub views: u64,
    pub estimated_minutes_watched: f64,
    pub average_view_duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsReport {
    pub channel_id: String,
    pub start_date: String,
    pub end_date: String,
    pub returned_end_date: Option<String>,
    pub views: u64,
    pub estimated_minutes_watched: f64,
    pub average_view_duration: f64,
    pub rows: Vec<AnalyticsRow>,
    pub fetched_at: String,
    pub timezone: String,
}

pub struct AnalyticsApi {
    client: Client,
    token: SecretString,
    channel_id: String,
}

fn error(code: &str, message: &str) -> AppError {
    AppError::new(code, message)
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

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .unwrap_or_default()
}

fn headers(value: &Value) -> Vec<String> {
    value["columnHeaders"]
        .as_array()
        .map(|headers| {
            headers
                .iter()
                .filter_map(|header| header["name"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn metric(row: &[Value], header_names: &[String], name: &str) -> f64 {
    header_names
        .iter()
        .position(|header| header == name)
        .and_then(|index| row.get(index))
        .map(number)
        .unwrap_or_default()
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
        let data: Value = response
            .json()
            .await
            .map_err(|_| error("YOUTUBE_ANALYTICS_RESPONSE", "YouTube 统计返回的数据无效"))?;
        if status.is_success() {
            return Ok(data);
        }
        let reason = field(&data, "/error/errors/0/reason");
        let (code, message) = match (status.as_u16(), reason.as_str()) {
            (401, _) | (_, "insufficientPermissions") => (
                "YOUTUBE_ANALYTICS_AUTH_REQUIRED",
                "需要统计权限，请在设置中重新授权 YouTube 频道",
            ),
            (_, "quotaExceeded" | "dailyLimitExceeded") => (
                "YOUTUBE_QUOTA_EXCEEDED",
                "YouTube 接口配额已用完，请稍后重试",
            ),
            (403, _) => (
                "YOUTUBE_ANALYTICS_FORBIDDEN",
                "YouTube 拒绝了统计请求，请检查频道权限和 API 配置",
            ),
            _ => (
                "YOUTUBE_ANALYTICS_FAILED",
                "读取 YouTube 统计失败，请稍后刷新重试",
            ),
        };
        Err(error(code, message))
    }

    pub async fn snapshot(&self) -> Result<ChannelAnalyticsSnapshot, AppError> {
        let data = self
            .request(
                &format!("{DATA_API}/channels"),
                &[("part", "statistics"), ("id", &self.channel_id)],
            )
            .await?;
        let item = items(&data)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| field(item, "/id") == self.channel_id)
            })
            .ok_or_else(|| error("YOUTUBE_CHANNEL_MISMATCH", "授权频道与所选频道不一致"))?;
        let view_count = item["statistics"]["viewCount"]
            .as_u64()
            .map(|value| value.to_string())
            .or_else(|| {
                item["statistics"]["viewCount"]
                    .as_str()
                    .map(ToOwned::to_owned)
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| error("YOUTUBE_ANALYTICS_RESPONSE", "YouTube 没有返回频道观看量"))?;
        Ok(ChannelAnalyticsSnapshot {
            channel_id: self.channel_id.clone(),
            view_count,
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        })
    }

    async fn report(
        &self,
        start_date: &str,
        end_date: &str,
        dimensions: Option<&str>,
    ) -> Result<Value, AppError> {
        let mut params = vec![
            ("ids", format!("channel=={}", self.channel_id)),
            ("startDate", start_date.to_owned()),
            ("endDate", end_date.to_owned()),
            (
                "metrics",
                "views,estimatedMinutesWatched,averageViewDuration".to_owned(),
            ),
            ("maxResults", "366".to_owned()),
        ];
        if let Some(dimensions) = dimensions {
            params.push(("dimensions", dimensions.to_owned()));
            params.push(("sort", "day".to_owned()));
        }
        let borrowed = params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>();
        self.request(&format!("{ANALYTICS_API}/reports"), &borrowed)
            .await
    }

    pub async fn report_range(
        &self,
        start_date: &str,
        end_date: &str,
    ) -> Result<AnalyticsReport, AppError> {
        validate_dates(start_date, end_date)?;
        let daily = self.report(start_date, end_date, Some("day")).await?;
        let summary = self.report(start_date, end_date, None).await?;
        let daily_headers = headers(&daily);
        let rows = daily["rows"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let values = row.as_array()?;
                        let date = values.first()?.as_str()?.to_owned();
                        Some(AnalyticsRow {
                            date,
                            views: metric(values, &daily_headers, "views").max(0.0) as u64,
                            estimated_minutes_watched: metric(
                                values,
                                &daily_headers,
                                "estimatedMinutesWatched",
                            ),
                            average_view_duration: metric(
                                values,
                                &daily_headers,
                                "averageViewDuration",
                            ),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let summary_headers = headers(&summary);
        let summary_values = summary["rows"]
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let returned_end_date = rows.last().map(|row| row.date.clone());
        Ok(AnalyticsReport {
            channel_id: self.channel_id.clone(),
            start_date: start_date.to_owned(),
            end_date: end_date.to_owned(),
            returned_end_date,
            views: metric(&summary_values, &summary_headers, "views").max(0.0) as u64,
            estimated_minutes_watched: metric(
                &summary_values,
                &summary_headers,
                "estimatedMinutesWatched",
            ),
            average_view_duration: metric(&summary_values, &summary_headers, "averageViewDuration"),
            rows,
            fetched_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            timezone: PACIFIC_TIMEZONE.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_invalid_or_too_wide_ranges() {
        assert!(validate_dates("2026-09-10", "2026-09-09").is_err());
        assert!(validate_dates("2025-01-01", "2026-01-03").is_err());
        assert!(validate_dates("2026-09-01", "2026-09-10").is_ok());
    }

    #[test]
    fn reads_metric_by_header_name_and_accepts_string_numbers() {
        let response = json!({
            "columnHeaders": [
                {"name": "day"},
                {"name": "averageViewDuration"},
                {"name": "views"}
            ]
        });
        let row = vec![json!("2026-09-09"), json!("42.5"), json!(17)];
        let names = headers(&response);
        assert_eq!(metric(&row, &names, "views"), 17.0);
        assert_eq!(metric(&row, &names, "averageViewDuration"), 42.5);
    }
}
