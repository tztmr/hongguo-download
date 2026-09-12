//! Opt-in live private upload smoke test. Never prints or exports credentials.
use hongguo_desktop_lib::youtube::{
    config::OAuthClientConfig,
    duplicates,
    models::UploadIntent,
    oauth::OAuthService,
    state::YouTubeStateStore,
    upload::{RefreshCallback, ResumableUploader, UploadCancellationToken},
    vault::OsTokenVault,
};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{}: {}", e.code, e.message);
        std::process::exit(1);
    }
}
async fn run() -> Result<(), hongguo_desktop_lib::app_error::AppError> {
    let args: Vec<String> = std::env::args().collect();
    let config_dir = PathBuf::from(args.get(1).expect("config directory"));
    let state = YouTubeStateStore::load(&config_dir)?.snapshot();
    let channel = state
        .active_channel_id
        .expect("an active channel is required");
    let oauth = Arc::new(OAuthService::new(
        OAuthClientConfig::load(&config_dir.join("youtube/oauth-client.json"))?,
        Arc::new(OsTokenVault),
    ));
    let token = oauth.access_token(&channel).await?;
    if args.get(2).map(String::as_str) == Some("--status") {
        let ids = args.get(3).expect("video ids");
        let response = reqwest::Client::new()
            .get("https://www.googleapis.com/youtube/v3/videos")
            .bearer_auth(token.expose_secret())
            .query(&[
                ("part", "snippet,status,processingDetails,contentDetails"),
                ("id", ids.as_str()),
            ])
            .send()
            .await
            .map_err(|_| {
                hongguo_desktop_lib::app_error::AppError::new(
                    "STATUS_NETWORK_ERROR",
                    "无法读取测试视频状态",
                )
            })?;
        if !response.status().is_success() {
            return Err(hongguo_desktop_lib::app_error::AppError::new(
                "STATUS_API_ERROR",
                "YouTube 视频状态查询失败",
            ));
        }
        let data = response.json::<serde_json::Value>().await.map_err(|_| {
            hongguo_desktop_lib::app_error::AppError::new("STATUS_JSON_ERROR", "视频状态响应无效")
        })?;
        if let Some(dest) = args.get(4) {
            std::fs::write(dest, serde_json::to_vec_pretty(&data).unwrap())
                .expect("status destination");
        }
        println!("{}", data);
        return Ok(());
    }
    let known = duplicates::channel_videos(&channel, &token).await?;
    println!(
        "{}",
        serde_json::json!({"authorized":true,"channelId":channel,"knownVideos":known.len()})
    );
    let Some(request_path) = args.get(2) else {
        return Ok(());
    };
    let intent: UploadIntent =
        serde_json::from_slice(&std::fs::read(request_path).expect("request file"))
            .expect("valid upload intent");
    assert!(
        matches!(
            intent.privacy_status,
            hongguo_desktop_lib::youtube::models::PrivacyStatus::Private
        ),
        "smoke uploads must be private"
    );
    if let Some(v) = known.iter().find(|v| v.title == intent.title) {
        println!("{}", serde_json::json!({"skippedExisting":v.video_id}));
        return Ok(());
    }
    let output = PathBuf::from(args.get(3).expect("output directory"));
    std::fs::create_dir_all(&output).expect("output directory");
    let refresh_channel = channel.clone();
    let refresh_oauth = oauth.clone();
    let refresh: RefreshCallback = Arc::new(move || {
        let oauth = refresh_oauth.clone();
        let channel = refresh_channel.clone();
        Box::pin(async move { oauth.access_token(&channel).await })
    });
    let mut result = ResumableUploader::new(output.join("checkpoints"))
        .upload(
            &intent,
            &channel,
            token,
            refresh,
            UploadCancellationToken::default(),
            Arc::new(|e| println!("upload {:?} {:.0}%", e.status, e.percent)),
        )
        .await?;
    std::fs::write(
        output.join(format!("{}-result.json", intent.job_id)),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .expect("save result");
    if let Some(cover) = intent.cover_path.as_ref() {
        let exe = std::env::current_exe().expect("executable");
        let tools =
            hongguo_desktop_lib::media::MediaTools::from_resource_root(exe.parent().unwrap())?;
        let temp = output.join("thumbnail");
        std::fs::create_dir_all(&temp).expect("thumbnail directory");
        let prepared =
            hongguo_desktop_lib::youtube::thumbnail::prepare_thumbnail(cover, &tools, &temp)?;
        let token = oauth.access_token(&channel).await?;
        hongguo_desktop_lib::youtube::thumbnail::set_thumbnail(
            &reqwest::Client::new(),
            &result.video_id,
            &prepared,
            &token,
        )
        .await?;
        result.thumbnail_state = hongguo_desktop_lib::youtube::models::ThumbnailState::Succeeded;
        std::fs::write(
            output.join(format!("{}-result.json", intent.job_id)),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .expect("save thumbnail result");
    }
    println!("{}", serde_json::to_string(&result).unwrap());
    let token = oauth.access_token(&channel).await?;
    let response = reqwest::Client::new()
        .get("https://www.googleapis.com/youtube/v3/videos")
        .bearer_auth(token.expose_secret())
        .query(&[
            ("part", "snippet,status,processingDetails,contentDetails"),
            ("id", result.video_id.as_str()),
        ])
        .send()
        .await;
    if let Ok(response) = response {
        if response.status().is_success() {
            if let Ok(data) = response.json::<serde_json::Value>().await {
                std::fs::write(
                    output.join(format!("{}-remote.json", intent.job_id)),
                    serde_json::to_vec_pretty(&data).unwrap(),
                )
                .expect("save remote status");
                println!("remote {}", data);
            }
        }
    }
    Ok(())
}
