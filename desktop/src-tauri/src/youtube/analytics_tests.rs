use super::*;
use serde_json::json;

async fn server(
    replies: Vec<(u16, Value)>,
) -> (AnalyticsApi, tokio::task::JoinHandle<Vec<String>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buf = [0; 8192];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with("GET "));
            assert!(request
                .to_lowercase()
                .contains("authorization: bearer analytics-test-token"));
            requests.push(request);
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (
        AnalyticsApi {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            token: SecretString::new("analytics-test-token"),
            channel_id: "c1".into(),
            data_base: base.clone(),
            analytics_base: base,
        },
        task,
    )
}
fn params(request: &str) -> std::collections::HashMap<String, String> {
    reqwest::Url::parse(&format!(
        "http://localhost{}",
        request.split_whitespace().nth(1).unwrap()
    ))
    .unwrap()
    .query_pairs()
    .map(|(key, value)| (key.into_owned(), value.into_owned()))
    .collect()
}
fn report(names: &[&str], rows: Value) -> Value {
    json!({"columnHeaders": names.iter().map(|name| json!({"name":name})).collect::<Vec<_>>(), "rows":rows})
}
fn daily() -> Value {
    report(
        &["views", "day", "averageViewDuration"],
        json!([[240, "2026-09-03", 42], [60, "2026-09-01", 120]]),
    )
}
fn totals() -> Value {
    report(
        &[
            "averageViewPercentage",
            "views",
            "subscribersLost",
            "averageViewDuration",
            "subscribersGained",
        ],
        json!([[125.4, 300, 7, 77.7, 5]]),
    )
}

#[test]
fn preserves_missing_and_invalid_metrics_without_shifting_unnamed_columns() {
    let data = json!({"columnHeaders":[{}, {"name":"views"}, {"name":"likes"}, {"name":"comments"}, {"name":"shares"}]});
    let row = vec![json!(900), json!(0), Value::Null, json!("NaN"), json!(-1)];
    let result = metrics(&row, &headers(&data));
    assert_eq!(result.views, Some(0.0));
    assert!(result.likes.is_none());
    assert!(result.comments.is_none());
    assert!(result.shares.is_none());
    assert!(result.engaged_views.is_none());
    assert_eq!(
        serde_json::to_value(result).unwrap()["engagedViews"],
        Value::Null
    );
    assert!(summary(&report(&["views"], json!([[1], [2]]))).is_err());
}

#[tokio::test]
async fn compares_equal_returned_periods_and_preserves_api_weighted_averages() {
    let (api, requests) = server(vec![
        (200, daily()),
        (200, totals()),
        (200, report(&["views"], json!([[200]]))),
    ])
    .await;
    let result = api
        .report_range("2026-09-01", "2026-09-10", Some("demoVideo01"))
        .await
        .unwrap();
    assert_eq!(result.returned_end_date.as_deref(), Some("2026-09-03"));
    assert_eq!(
        result
            .rows
            .iter()
            .map(|row| row.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-09-01", "2026-09-03"]
    );
    assert_eq!(result.metrics.average_view_duration, Some(77.7));
    assert_eq!(result.metrics.average_view_percentage, Some(125.4));
    assert_eq!(
        result.metrics.subscribers_gained.unwrap() - result.metrics.subscribers_lost.unwrap(),
        -2.0
    );
    let previous = result.comparison.unwrap();
    assert_eq!(previous.start_date, "2026-08-29");
    assert_eq!(previous.end_date, "2026-08-31");
    let requests = requests.await.unwrap();
    let queries = requests
        .iter()
        .map(|request| params(request))
        .collect::<Vec<_>>();
    for query in &queries {
        assert_eq!(query["ids"], "channel==c1");
        assert_eq!(query["filters"], "video==demoVideo01");
        assert!(query["metrics"].contains("engagedViews"));
        assert!(query["metrics"].contains("subscribersLost"));
    }
    assert_eq!(queries[0]["dimensions"], "day");
    assert_eq!(queries[0]["sort"], "day");
    assert_eq!(queries[0]["maxResults"], "367");
    assert_eq!(queries[1]["endDate"], "2026-09-03");
    assert!(!queries[1].contains_key("dimensions"));
    assert_eq!(queries[2]["startDate"], "2026-08-29");
    assert_eq!(queries[2]["endDate"], "2026-08-31");
}

#[tokio::test]
async fn keeps_current_metrics_when_comparison_fails() {
    let (api, requests) = server(vec![
        (200, daily()),
        (200, totals()),
        (
            403,
            json!({"error":{"errors":[{"reason":"quotaExceeded"}]}}),
        ),
    ])
    .await;
    let result = api
        .report_range("2026-09-01", "2026-09-10", None)
        .await
        .unwrap();
    assert_eq!(result.metrics.views, Some(300.0));
    assert!(result.comparison.is_none());
    assert_eq!(result.warnings[0].code, "YOUTUBE_QUOTA_EXCEEDED");
    assert!(requests
        .await
        .unwrap()
        .iter()
        .all(|request| !params(request).contains_key("filters")));
}

#[tokio::test]
async fn empty_daily_report_does_not_invent_zero_totals_or_query_comparison() {
    let (api, requests) = server(vec![(200, report(&["day", "views"], Value::Null))]).await;
    let result = api
        .report_range("2026-09-01", "2026-09-10", None)
        .await
        .unwrap();
    assert!(result.rows.is_empty());
    assert!(result.returned_end_date.is_none());
    assert!(result.metrics.views.is_none());
    assert!(result.comparison.is_none());
    assert_eq!(requests.await.unwrap().len(), 1);
}

#[tokio::test]
async fn rejects_out_of_scope_and_duplicate_daily_dates() {
    for rows in [
        json!([["2026-08-31", 10]]),
        json!([["2026-09-11", 10]]),
        json!([["2026-09-02", 10], ["2026-09-02", 20]]),
    ] {
        let (api, requests) = server(vec![(200, report(&["day", "views"], rows))]).await;
        assert_eq!(
            api.report_range("2026-09-01", "2026-09-10", None)
                .await
                .unwrap_err()
                .code,
            "YOUTUBE_ANALYTICS_RESPONSE"
        );
        assert_eq!(requests.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn selects_supported_metrics_for_each_audience_dimension_and_video_scope() {
    for (kind, dimension, averages) in [
        (BreakdownKind::Traffic, "insightTrafficSourceType", false),
        (BreakdownKind::Device, "deviceType", false),
        (BreakdownKind::Country, "country", true),
        (BreakdownKind::ContentType, "creatorContentType", true),
        (BreakdownKind::Subscribed, "subscribedStatus", true),
    ] {
        let (api, requests) = server(vec![(
            200,
            report(
                &[
                    dimension,
                    "views",
                    "engagedViews",
                    "estimatedMinutesWatched",
                ],
                json!([["SHORTS", 30, 20, 10]]),
            ),
        )])
        .await;
        let result = api
            .breakdown("2026-09-01", "2026-09-10", kind, Some("demoVideo01"))
            .await
            .unwrap();
        assert_eq!(result.rows[0].metrics.views, Some(30.0));
        assert!(result.rows[0].metrics.likes.is_none());
        let query = params(&requests.await.unwrap()[0]);
        assert_eq!(query["ids"], "channel==c1");
        assert_eq!(query["dimensions"], dimension);
        assert_eq!(query["filters"], "video==demoVideo01");
        assert_eq!(query["metrics"].contains("averageViewDuration"), averages);
        assert_eq!(query["metrics"].contains("averageViewPercentage"), averages);
        assert!(!query["metrics"].contains("likes"));
    }
}

#[tokio::test]
async fn paginates_country_rows_after_200_and_rejects_repeated_pages() {
    let page = report(
        &["country", "views"],
        json!((0..200)
            .map(|index| json!([format!("region-{index}"), 1]))
            .collect::<Vec<_>>()),
    );
    let (api, requests) = server(vec![
        (200, page.clone()),
        (200, report(&["country", "views"], json!([["TW", 3]]))),
    ])
    .await;
    let result = api
        .breakdown("2026-09-01", "2026-09-10", BreakdownKind::Country, None)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 201);
    assert!(!result.truncated);
    assert_eq!(params(&requests.await.unwrap()[1])["startIndex"], "201");
    let (api, requests) = server(vec![(200, page.clone()), (200, page)]).await;
    assert_eq!(
        api.breakdown("2026-09-01", "2026-09-10", BreakdownKind::Country, None)
            .await
            .unwrap_err()
            .code,
        "YOUTUBE_ANALYTICS_RESPONSE"
    );
    assert_eq!(requests.await.unwrap().len(), 2);
}

#[tokio::test]
async fn caps_top_videos_at_200_and_fetches_titles_in_owner_checked_batches_of_50() {
    let rows = (0..200)
        .map(|index| {
            json!([
                200 - index,
                if index % 2 == 0 {
                    "SHORTS"
                } else {
                    "VIDEO_ON_DEMAND"
                },
                format!("demo{index:07}")
            ])
        })
        .collect::<Vec<_>>();
    let mut replies = vec![(
        200,
        report(&["views", "creatorContentType", "video"], json!(rows)),
    )];
    for batch in 0..4 {
        replies.push((200, json!({"items": (batch*50..(batch+1)*50).map(|index| json!({"id":format!("demo{index:07}"),"snippet":{"channelId":if index == 0 {"other"} else {"c1"},"title":format!("视频 {index}")}})).collect::<Vec<_>>()})));
    }
    let (api, requests) = server(replies).await;
    let result = api
        .breakdown("2026-09-01", "2026-09-10", BreakdownKind::Videos, None)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 200);
    assert!(result.truncated);
    assert_eq!(result.rows[0].content_type.as_deref(), Some("SHORTS"));
    assert!(result.rows[0].title.is_none());
    assert_eq!(result.rows[199].title.as_deref(), Some("视频 199"));
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 5);
    let query = params(&requests[0]);
    assert_eq!(query["dimensions"], "video,creatorContentType");
    assert_eq!(query["sort"], "-views");
    assert_eq!(query["maxResults"], "200");
    for request in &requests[1..] {
        let query = params(request);
        assert_eq!(query["part"], "snippet");
        assert_eq!(query["id"].split(',').count(), 50);
    }
}

#[tokio::test]
async fn keeps_video_metrics_when_title_lookup_fails() {
    let (api, requests) = server(vec![
        (
            200,
            report(
                &["video", "views", "creatorContentType"],
                json!([["demoVideo01", 42, "SHORTS"]]),
            ),
        ),
        (
            403,
            json!({"error":{"errors":[{"reason":"quotaExceeded"}]}}),
        ),
    ])
    .await;
    let result = api
        .breakdown("2026-09-01", "2026-09-10", BreakdownKind::Videos, None)
        .await
        .unwrap();
    assert_eq!(result.rows[0].metrics.views, Some(42.0));
    assert!(result.rows[0].title.is_none());
    assert_eq!(result.warnings[0].code, "YOUTUBE_QUOTA_EXCEEDED");
    assert_eq!(requests.await.unwrap().len(), 2);
}

#[tokio::test]
async fn snapshot_matches_channel_and_keeps_hidden_or_missing_counts_unknown() {
    let (api, requests) = server(vec![(200, json!({"items":[
        {"id":"other","statistics":{"viewCount":"1"}},
        {"id":"c1","statistics":{"viewCount":"9007199254740993","subscriberCount":"200","hiddenSubscriberCount":true}}]})),
        (200, json!({"items":[{"id":"other","statistics":{"viewCount":"1"}}]}))]).await;
    let result = api.snapshot().await.unwrap();
    assert_eq!(result.view_count, "9007199254740993");
    assert!(result.hidden_subscriber_count);
    assert!(result.subscriber_count.is_none());
    assert!(result.video_count.is_none());
    assert_eq!(
        api.snapshot().await.unwrap_err().code,
        "YOUTUBE_CHANNEL_MISMATCH"
    );
    for request in requests.await.unwrap() {
        assert_eq!(params(&request)["id"], "c1");
    }
}

#[tokio::test]
async fn rejects_invalid_video_filters_before_any_network_request() {
    let (api, requests) = server(vec![]).await;
    assert_eq!(
        api.report_range("2026-09-01", "2026-09-10", Some("id;country==US"))
            .await
            .unwrap_err()
            .code,
        "YOUTUBE_ANALYTICS_INVALID_VIDEO"
    );
    assert_eq!(
        api.breakdown(
            "2026-09-01",
            "2026-09-10",
            BreakdownKind::Videos,
            Some("demoVideo01")
        )
        .await
        .unwrap_err()
        .code,
        "YOUTUBE_ANALYTICS_INVALID_VIDEO"
    );
    assert_eq!(requests.await.unwrap().len(), 0);
}

#[test]
fn classifies_analytics_service_configuration_errors_separately() {
    let (code, message) = classify_api_error(403, "serviceDisabled");

    assert_eq!(code, "YOUTUBE_ANALYTICS_API_NOT_ENABLED");
    assert!(message.contains("YouTube Analytics API"));
    assert!(message.contains("无需再次授权"));
}

#[test]
fn keeps_missing_analytics_scope_as_reauthorization_error() {
    let (code, message) = classify_api_error(403, "insufficientPermissions");

    assert_eq!(code, "YOUTUBE_ANALYTICS_AUTH_REQUIRED");
    assert!(message.contains("重新授权"));
}

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
    assert_eq!(metric(&row, &names, "views"), Some(17.0));
    assert_eq!(metric(&row, &names, "averageViewDuration"), Some(42.5));
}
