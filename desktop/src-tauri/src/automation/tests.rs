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
