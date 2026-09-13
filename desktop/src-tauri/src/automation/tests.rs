use super::*;
use serde_json::json;
use std::fs;
fn config() -> Value {
    json!({"uploadFormat":"auto","firstEpisodeShorts":false,"interval":"5","types":["真人剧"],"scope":"all","orientation":"all","keywords":"","exclude":"","completeOnly":true,"definition":"auto","concurrency":"1","separate":false,"subtitles":false,"subtitleSource":"original","subtitleFormat":"srt","retries":"3","channel":"channel-a","privacy":"private","title":"{剧名}","description":"{简介}","tags":"{剧名}","coverSource":"source","metadataSource":"template","category":"24","imageMode":"reference","minDisk":"20","resume":true,"notify":false})
}
fn task() -> Task {
    Task::new(
        source::Candidate {
            book_id: "123".into(),
            title: "故事第二季".into(),
            cover: String::new(),
            summary: "简介".into(),
            category: String::new(),
            tags: vec![],
            episode_count: 2,
            online_time: None,
            content_type: 1,
            release_type: "playlet".into(),
            complete: Some(true),
        },
        config(),
        PathBuf::from("/tmp/automation-test"),
    )
}
fn temporary() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "auto-tests-{}-{}",
        std::process::id(),
        model::fingerprint(&format!("{:?}", std::time::SystemTime::now()))
    ));
    fs::create_dir_all(&root).unwrap();
    root.join("state.json")
}

#[test]
fn one_slot_pipelines_download_merge_ai_and_upload_without_starvation() {
    let mut jobs = vec![];
    for (id, stage) in [
        ("upload", "upload"),
        ("merge", "merge"),
        ("separate", "separate"),
        ("download", "download"),
        ("next-download", "download"),
    ] {
        let mut job = task();
        job.id = id.into();
        job.stage = stage.into();
        jobs.push(job);
    }
    let state = Snapshot {
        config: Some(config()),
        mode: Mode::Running,
        jobs,
        ..Default::default()
    };
    let ready = pipeline::select(&state, &HashSet::from(["upload".into()]), now());
    assert_eq!(ready, ["merge", "separate", "download"]);
}

#[test]
fn pipeline_bounds_download_backlog_but_keeps_resuming_existing_downloads() {
    let mut jobs = vec![];
    for i in 0..6 {
        let mut job = task();
        job.id = format!("cooldown-{i}");
        job.stage = "upload".into();
        job.retry_at = now() + 900;
        job.files.push(PathBuf::from("ready.mp4"));
        jobs.push(job);
    }
    let mut download = task();
    download.id = "incoming".into();
    download.stage = "download".into();
    jobs.push(download);
    let mut state = Snapshot {
        config: Some(config()),
        mode: Mode::Running,
        jobs,
        ..Default::default()
    };
    assert!(pipeline::select(&state, &HashSet::new(), now()).is_empty());
    state
        .jobs
        .last_mut()
        .unwrap()
        .files
        .push(PathBuf::from("already-downloaded-episode.mp4"));
    assert_eq!(
        pipeline::select(&state, &HashSet::new(), now()),
        ["incoming"]
    );
    state.mode = Mode::Paused;
    assert!(pipeline::select(&state, &HashSet::new(), now()).is_empty());
}
#[test]
fn validates_settings_and_never_persists_unknown_secret_fields() {
    let mut c = config();
    c["apiKey"] = json!("synthetic-not-a-real-key");
    c["secretUpdates"] = json!({"image":"secret"});
    let clean = validate_config(c).unwrap();
    assert!(clean.get("apiKey").is_none());
    assert!(clean.get("secretUpdates").is_none());
    assert!(clean["duplicate"].as_bool().unwrap());
    for (key, value) in [
        ("channel", json!("")),
        ("types", json!([])),
        ("concurrency", json!("9")),
        ("privacy", json!("invalid")),
    ] {
        let mut c = config();
        c[key] = value;
        assert!(validate_config(c).is_err(), "{key}");
    }
}
#[test]
fn season_and_channel_make_distinct_stable_job_and_upload_identities() {
    let a = task();
    let b = Task::new(a.source.clone(), a.config.clone(), a.root.clone());
    assert_eq!(a.id, b.id);
    let mut source = a.source.clone();
    source.title = "故事第三季".into();
    let other = Task::new(source, a.config.clone(), PathBuf::from("/tmp"));
    assert_ne!(a.id, other.id);
    let mut c = a.config.clone();
    c["channel"] = json!("channel-b");
    assert_ne!(
        Task::new(a.source.clone(), c, PathBuf::from("/tmp")).id,
        a.id
    );
    assert_ne!(a.upload_id(false), a.upload_id(true));
    assert!(a.root.to_string_lossy().contains("__123__S2__"));
}
#[test]
fn restart_retains_side_effect_handles_and_respects_resume_policy() {
    let path = temporary();
    let mut job = task();
    job.status = Status::Working;
    job.stage = "upload".into();
    job.merge_job = Some("merge-durable".into());
    job.main_video_url = "https://www.youtube.com/watch?v=done".into();
    let mut c = config();
    c["resume"] = json!(false);
    let saved = Snapshot {
        config: Some(c),
        mode: Mode::Running,
        jobs: vec![job.clone()],
        ..Default::default()
    };
    storage::save(&path, &saved).unwrap();
    let runtime = Service::load(path.clone()).unwrap();
    let s = runtime.snapshot();
    assert_eq!(s.mode, Mode::Paused);
    assert_eq!(s.jobs[0].status, Status::Pending);
    assert_eq!(s.jobs[0].merge_job, job.merge_job);
    assert_eq!(s.jobs[0].upload_id(false), job.upload_id(false));
    assert_eq!(s.jobs[0].main_video_url, job.main_video_url);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn startup_preserves_cached_key_status_without_opening_the_system_vault() {
    let path = temporary();
    let saved = Snapshot {
        config: Some(config()),
        key_status: KeyStatus {
            text: true,
            image: true,
        },
        ..Default::default()
    };
    storage::save(&path, &saved).unwrap();
    let snapshot = Service::load(path.clone()).unwrap().snapshot();
    assert!(snapshot.key_status.text && snapshot.key_status.image);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn pause_stop_and_review_persist_without_dropping_completed_work() {
    let path = temporary();
    let runtime = Service::load(path.clone()).unwrap();
    let mut job = task();
    job.status = Status::Review;
    job.files.push(PathBuf::from("/tmp/existing.mp4"));
    runtime
        .transaction(|s| {
            s.jobs.push(job.clone());
            s.mode = Mode::Running;
            Ok(())
        })
        .unwrap();
    assert_eq!(runtime.control("pause").unwrap().mode, Mode::Paused);
    let s = runtime.review(&job.id, "continue").unwrap();
    assert_eq!(s.jobs[0].status, Status::Pending);
    assert!(s.jobs[0].allow_duplicate);
    runtime.control("stop").unwrap();
    let disk = storage::load(&path).unwrap();
    assert_eq!(disk.mode, Mode::Stopped);
    assert_eq!(disk.jobs[0].files, job.files);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn corrupt_state_does_not_silently_start_fresh_or_erase_history() {
    let path = temporary();
    fs::write(&path, b"broken").unwrap();
    assert!(Service::load(path.clone()).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"broken");
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn automatic_retry_keeps_running_after_quick_retries_without_manual_action() {
    let mut job = task();
    job.stage = "separate".into();
    job.files.push(PathBuf::from("downloaded.mp4"));
    job.separate_job = Some("existing-separation".into());
    for attempt in [1, 2, 3, 4, 50, u64::MAX] {
        job.attempts = attempt;
        let before = now();
        job.defer_retry();
        assert!(!job.terminal());
        assert!(job.retry_ready);
        assert!(job.retry_at > before && job.retry_at <= now() + 900);
        assert_eq!(job.files.len(), 1);
        assert_eq!(job.separate_job.as_deref(), Some("existing-separation"));
    }
    job.config["retries"] = json!("0");
    job.attempts = 1;
    job.defer_retry();
    assert_eq!(job.status, Status::Observing);
    assert!(job.retry_at >= now() + 899);
}

#[test]
fn upgrade_automatically_requeues_failed_media_but_respects_stop_and_review() {
    let path = temporary();
    let mut failed = task();
    failed.status = Status::Failed;
    failed.stage = "merge".into();
    failed.merge_job = Some("old-copy-only".into());
    failed.files.push(PathBuf::from("all-152-downloaded.mp4"));
    let mut review = task();
    review.id = "review".into();
    review.status = Status::Review;
    let saved = Snapshot {
        config: Some(config()),
        mode: Mode::Stopped,
        jobs: vec![failed.clone(), review],
        ..Default::default()
    };
    storage::save(&path, &saved).unwrap();
    let state = Service::load(path.clone()).unwrap().snapshot();
    assert_eq!(state.mode, Mode::Stopped);
    assert_eq!(state.jobs[0].status, Status::Pending);
    assert!(state.jobs[0].retry_ready);
    assert_eq!(state.jobs[0].files, failed.files);
    assert_eq!(state.jobs[0].merge_job, failed.merge_job);
    assert_eq!(state.jobs[1].status, Status::Review);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn validates_ordered_cover_models_and_preserves_legacy_model() {
    let mut c = config();
    c["coverSource"] = json!("moyuu");
    c["coverModel"] = json!("gpt-image-2");
    c["coverModels"] = json!(["gpt-image-2-medium", "gpt-image-2", "gpt-image-2-medium"]);
    let clean = validate_config(c.clone()).unwrap();
    assert_eq!(
        clean["coverModels"],
        json!(["gpt-image-2-medium", "gpt-image-2"])
    );
    for invalid in [
        json!("gpt-image-2"),
        json!([null]),
        json!([" "]),
        json!(["a", "b", "c", "d", "e"]),
    ] {
        c["coverModels"] = invalid;
        assert!(validate_config(c.clone()).is_err());
    }
    c.as_object_mut().unwrap().remove("coverModels");
    assert_eq!(validate_config(c).unwrap()["coverModel"], "gpt-image-2");
}
