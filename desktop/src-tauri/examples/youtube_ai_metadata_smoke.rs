//! Explicit live update of existing private smoke videos; credentials remain in the OS vault.
use hongguo_desktop_lib::{
    app_error::AppError,
    youtube::{
        config::OAuthClientConfig, oauth::OAuthService, state::YouTubeStateStore,
        vault::OsTokenVault,
    },
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
fn error(msg: &str) -> AppError {
    AppError::new("AI_SMOKE_FAILED", msg)
}
async fn checked(response: reqwest::Response) -> Result<Value, AppError> {
    let status = response.status();
    if !status.is_success() {
        return Err(error(&format!("YouTube HTTP {}", status.as_u16())));
    }
    response.json().await.map_err(|_| error("响应 JSON 无效"))
}
fn tag_set(value: &Value) -> std::collections::BTreeSet<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{}: {}", e.code, e.message);
        std::process::exit(1);
    }
}
async fn run() -> Result<(), AppError> {
    let args: Vec<String> = std::env::args().collect();
    let config = PathBuf::from(args.get(1).expect("config dir"));
    let request: Value = serde_json::from_slice(
        &std::fs::read(args.get(2).expect("request")).expect("read request"),
    )
    .expect("request JSON");
    let output = PathBuf::from(args.get(3).expect("output"));
    let channel = YouTubeStateStore::load(&config)?
        .snapshot()
        .active_channel_id
        .ok_or(error("没有活动频道"))?;
    let oauth = OAuthService::new(
        OAuthClientConfig::load(&config.join("youtube/oauth-client.json"))?,
        Arc::new(OsTokenVault),
    );
    let token = oauth.access_token(&channel).await?;
    let client = reqwest::Client::new();
    let mut results = vec![];
    for update in request.as_array().ok_or(error("请求必须是列表"))? {
        let id = update["id"].as_str().ok_or(error("缺少视频 ID"))?;
        let title = update["title"].as_str().ok_or(error("缺少标题"))?;
        let desc = update["description"].as_str().ok_or(error("缺少描述"))?;
        if title.is_empty()
            || title.chars().count() > 100
            || desc.len() > 5000
            || title.contains(['<', '>'])
            || desc.contains(['<', '>'])
        {
            return Err(error("文案长度或字符无效"));
        }
        let current = checked(
            client
                .get("https://www.googleapis.com/youtube/v3/videos")
                .bearer_auth(token.expose_secret())
                .query(&[("part", "snippet,status"), ("id", id)])
                .send()
                .await
                .map_err(|_| error("读取视频网络错误"))?,
        )
        .await?;
        let item = &current["items"][0];
        if item["snippet"]["channelId"] != channel || item["status"]["privacyStatus"] != "private" {
            return Err(error("仅允许更新当前频道的私享测试视频"));
        }
        let mut snippet = json!({});
        for field in [
            "title",
            "description",
            "categoryId",
            "tags",
            "defaultLanguage",
            "defaultAudioLanguage",
        ] {
            if let Some(v) = item["snippet"].get(field) {
                snippet[field] = v.clone();
            }
        }
        snippet["title"] = json!(title);
        snippet["description"] = json!(desc);
        snippet["tags"] = update["tags"].clone();
        if !snippet["categoryId"].is_string() || !snippet["tags"].is_array() {
            return Err(error("分类或标签无效"));
        }
        if item["snippet"]["title"] != title
            || item["snippet"]["description"] != desc
            || tag_set(&item["snippet"]["tags"]) != tag_set(&update["tags"])
        {
            checked(
                client
                    .put("https://www.googleapis.com/youtube/v3/videos")
                    .bearer_auth(token.expose_secret())
                    .header(
                        "If-Match",
                        item["etag"].as_str().ok_or(error("缺少版本信息"))?,
                    )
                    .query(&[("part", "snippet")])
                    .json(&json!({"id":id,"snippet":snippet}))
                    .send()
                    .await
                    .map_err(|_| error("更新文案网络错误"))?,
            )
            .await?;
        }
        let mut thumbnail = json!({"attempted":false});
        if let Some(path) = update["coverPath"].as_str() {
            let exe = std::env::current_exe().map_err(|_| error("程序目录无效"))?;
            let tools =
                hongguo_desktop_lib::media::MediaTools::from_resource_root(exe.parent().unwrap())?;
            let temp = output.parent().unwrap().join("ai-thumbnail");
            std::fs::create_dir_all(&temp).map_err(|_| error("缩略图目录无效"))?;
            let prepared = hongguo_desktop_lib::youtube::thumbnail::prepare_thumbnail(
                &PathBuf::from(path),
                &tools,
                &temp,
            )?;
            thumbnail = match hongguo_desktop_lib::youtube::thumbnail::set_thumbnail(
                &client, id, &prepared, &token,
            )
            .await
            {
                Ok(()) => json!({"attempted":true,"succeeded":true}),
                Err(e) => {
                    json!({"attempted":true,"succeeded":false,"code":e.code,"message":e.message})
                }
            };
        }
        std::fs::write(
            output.with_file_name(format!("{id}-ai-thumbnail-attempt.json")),
            serde_json::to_vec_pretty(&thumbnail).unwrap(),
        )
        .map_err(|_| error("保存缩略图证据失败"))?;
        let mut verified = None;
        for attempt in 0..5 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            let remote = checked(
                client
                    .get("https://www.googleapis.com/youtube/v3/videos")
                    .bearer_auth(token.expose_secret())
                    .query(&[
                        ("part", "snippet,status,processingDetails,contentDetails"),
                        ("id", id),
                    ])
                    .send()
                    .await
                    .map_err(|_| error("回读视频网络错误"))?,
            )
            .await?;
            let v = &remote["items"][0];
            if v["snippet"]["title"] != title
                || v["snippet"]["description"] != desc
                || tag_set(&v["snippet"]["tags"]) != tag_set(&update["tags"])
                || v["status"]["privacyStatus"] != "private"
            {
                continue;
            }
            verified = Some(v.clone());
            break;
        }
        let v = verified.ok_or(error("更新后回读不一致，请稍后只读核对"))?;
        let result = json!({"id":id,"metadataVerified":true,"thumbnail":thumbnail,"remote":v});
        results.push(result);
        std::fs::write(&output, serde_json::to_vec_pretty(&results).unwrap())
            .map_err(|_| error("保存测试证据失败"))?;
        println!(
            "{}",
            json!({"id":id,"title":title,"metadataVerified":true,"thumbnail":thumbnail})
        );
    }
    Ok(())
}
