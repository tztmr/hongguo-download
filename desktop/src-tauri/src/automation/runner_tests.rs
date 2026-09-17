use super::super::source::Candidate;
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    base: PathBuf,
    task: Task,
}

fn mp4_envelope() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&12u32.to_be_bytes());
    bytes.extend_from_slice(b"moovtest");
    bytes.extend_from_slice(&12u32.to_be_bytes());
    bytes.extend_from_slice(b"mdattest");
    bytes
}

#[test]
fn recovered_download_writes_receipt_and_preserves_file_on_probe_tool_failure() {
    let f = Fixture::new();
    let path = f.file("0001_episode_1080p.mp4", &mp4_envelope());
    let receipt = f.task.root.join("下载完成-1.json");
    let error = recover_download_file(&f.task.root, &path, &receipt, |_| {
        Err(AppError::new("MEDIA_TOOL_MISSING", "缺少检测工具"))
    })
    .unwrap_err();
    assert_eq!(error.code, "MEDIA_TOOL_MISSING");
    assert!(path.exists());
    assert!(!receipt.exists());
    let record = recover_download_file(&f.task.root, &path, &receipt, |_| Ok(()))
        .unwrap()
        .unwrap();
    let saved: OwnedFile = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(record.hash, saved.hash);
    assert_eq!(record.path, fs::canonicalize(&path).unwrap());
    assert_eq!(record.size, 24);
}

#[test]
fn unsupported_download_is_preserved_and_replaced_by_a_validated_compatible_episode() {
    let f = Fixture::new();
    let path = f.file("0003_7678806141064711230_720p.mp4", &mp4_envelope());
    let previous = f.file(
        "0003_7678806141064711230_720p.mp4.unsupported-0",
        b"keep previous file",
    );
    let receipt = f.task.root.join("下载完成-3.json");
    let result = recover_download_file(&f.task.root, &path, &receipt, |_| {
        Err(AppError::new("MEDIA_CODEC_UNSUPPORTED", "ByteVC2 不支持"))
    })
    .unwrap();
    assert!(result.is_none());
    assert!(!path.exists());
    assert!(!receipt.exists());
    assert_eq!(fs::read(previous).unwrap(), b"keep previous file");
    let retained = path.with_extension("mp4.unsupported-1");
    assert_eq!(fs::read(retained).unwrap(), mp4_envelope());

    let compatible = f.file("0003_7678806141064711230_1080p.mp4", &mp4_envelope());
    let record = recover_download_file(&f.task.root, &compatible, &receipt, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(record.path, fs::canonicalize(compatible).unwrap());
    let saved: OwnedFile = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(saved.hash, record.hash);
}

#[test]
fn unknown_probe_output_is_not_mistaken_for_a_known_unsupported_codec() {
    let f = Fixture::new();
    let path = f.file("0003_episode_720p.mp4", &mp4_envelope());
    let receipt = f.task.root.join("下载完成-3.json");
    let error = recover_download_file(&f.task.root, &path, &receipt, |_| {
        Err(AppError::new("FFPROBE_INVALID", "检测工具返回无效 JSON"))
    })
    .unwrap_err();
    assert_eq!(error.code, "FFPROBE_INVALID");
    assert!(path.exists());
    assert!(!receipt.exists());
}

#[test]
fn truncated_unreceipted_download_is_preserved_without_adoption_or_cleanup() {
    let mut f = Fixture::new();
    let path = f.file("0001_episode_1080p.mp4", b"partial MP4");
    let receipt = f.task.root.join("下载完成-1.json");
    assert!(
        recover_download_file(&f.task.root, &path, &receipt, |_| panic!(
            "must reject truncation before probing"
        ))
        .unwrap()
        .is_none()
    );
    assert!(!path.exists());
    assert!(!receipt.exists());
    let retained = path.with_extension("mp4.incomplete-0");
    assert_eq!(fs::read(&retained).unwrap(), b"partial MP4");
    f.file(
        "自动追剧记录.json",
        &serde_json::to_vec(&json!({"id":f.task.id,"bookId":f.task.book_id})).unwrap(),
    );
    cleanup(&mut f.task).unwrap();
    assert!(retained.exists());
}

#[test]
fn resumed_download_returns_to_first_missing_episode_and_keeps_later_files() {
    let mut f = Fixture::new();
    let first = f.file("0001_first_1080p.mp4", &mp4_envelope());
    let missing = f.task.root.join("0002_second_1080p.mp4");
    let third = f.file("0003_third_1080p.mp4", &mp4_envelope());
    f.task.files = vec![first.clone(), missing, third.clone()];
    f.task.episode_done = 3;
    reconcile_download_files(&mut f.task).unwrap();
    assert_eq!(f.task.files, vec![first]);
    assert_eq!(f.task.episode_done, 1);
    assert!(third.exists());
}

#[test]
fn automated_privacy_uses_setting_and_defaults_private_regardless_of_cover() {
    let mut fixture = Fixture::new();
    fixture
        .task
        .config
        .as_object_mut()
        .unwrap()
        .remove("privacy");
    assert_eq!(automated_privacy(&fixture.task), PrivacyStatus::Private);
    fixture.task.cover = Some(fixture.file("生成封面.png", b"generated cover"));
    assert_eq!(automated_privacy(&fixture.task), PrivacyStatus::Private);
    for (setting, expected) in [
        ("private", PrivacyStatus::Private),
        ("unlisted", PrivacyStatus::Unlisted),
        ("public", PrivacyStatus::Public),
    ] {
        fixture.task.config["privacy"] = json!(setting);
        assert_eq!(automated_privacy(&fixture.task), expected);
        fixture.task.cover = None;
        assert_eq!(automated_privacy(&fixture.task), expected);
    }
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "hongguo-auto-cleanup-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        // macOS temp_dir can contain /var, whose ancestor is a symlink.
        let base = fs::canonicalize(base).unwrap();
        let source: Candidate = serde_json::from_value(json!({
            "bookId": "fixture-book", "title": "测试剧第二季", "cover": "", "summary": "",
            "category": "", "tags": [], "episodeCount": 2, "onlineTime": null,
            "contentType": 1, "releaseType": "playlet", "complete": true
        }))
        .unwrap();
        let mut task = Task::new(
            source,
            json!({
                "channel": "fixture-channel", "firstEpisodeShorts": false,
                "deleteEpisodes": true, "deleteFinal": true, "keepSubtitles": true
            }),
            base.clone(),
        );
        fs::create_dir_all(&task.root).unwrap();
        task.main_done = true;
        task.stage = "cleanup".into();
        Self { base, task }
    }

    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.task.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn owned(&mut self, relative: &str) -> PathBuf {
        let path = self.file(relative, b"fixture media contents");
        own(&mut self.task, &path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn successful_cleanup_removes_owned_receipts_empty_directories_and_task_root() {
    let mut f = Fixture::new();
    let episode = f.owned("0001.mp4");
    f.task.files.push(episode.clone());
    f.owned("合并视频/成片.mp4");
    let marker = json!({"id": f.task.id, "bookId": f.task.book_id});
    f.file("自动追剧记录.json", &serde_json::to_vec(&marker).unwrap());
    f.file(
        "下载完成-1.json",
        &serde_json::to_vec(&identity(&episode).unwrap()).unwrap(),
    );
    f.file(
        "发布文案.json",
        &serde_json::to_vec(&json!({"taskId":f.task.id})).unwrap(),
    );
    cleanup(&mut f.task).unwrap();
    assert!(
        !f.task.root.exists(),
        "completed task folder should be removed"
    );
    assert!(f.base.exists());
    assert!(!f.task.id.is_empty(), "central dedup identity must remain");
    cleanup(&mut f.task).unwrap();
}

#[test]
fn full_cleanup_archives_subtitles_and_preserves_untracked_files() {
    for unrelated in [false, true] {
        let mut f = Fixture::new();
        let subtitle = f.owned("字幕/整季/字幕.srt");
        f.task.subtitle = Some(subtitle.clone());
        f.file(
            "自动追剧记录.json",
            &serde_json::to_vec(&json!({"id":f.task.id,"bookId":f.task.book_id})).unwrap(),
        );
        let unknown = if unrelated {
            Some(f.file("我的笔记.txt", b"user notes"))
        } else {
            None
        };
        let summary = cleanup(&mut f.task).unwrap();
        assert!(!subtitle.exists());
        let saved = f.task.subtitle.as_ref().unwrap();
        assert!(saved.starts_with(f.task.root.parent().unwrap().join("字幕留存")));
        assert_eq!(fs::read(saved).unwrap(), b"fixture media contents");
        if let Some(unknown) = unknown {
            assert_eq!(fs::read(unknown).unwrap(), b"user notes");
            assert!(summary.contains("未登记文件"));
        } else {
            assert!(!f.task.root.exists());
            assert!(summary.contains("任务文件夹已删除"));
        }
        cleanup(&mut f.task).unwrap();
    }
}

#[test]
fn archive_collision_keeps_the_source_subtitle_and_existing_archive() {
    let mut f = Fixture::new();
    let source = f.owned("字幕.srt");
    f.file(
        "自动追剧记录.json",
        &serde_json::to_vec(&json!({"id":f.task.id,"bookId":f.task.book_id})).unwrap(),
    );
    let target = f
        .task
        .root
        .parent()
        .unwrap()
        .join("字幕留存")
        .join(f.task.root.file_name().unwrap())
        .join("字幕.srt");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"existing subtitle").unwrap();
    assert!(cleanup(&mut f.task).is_err());
    assert_eq!(fs::read(source).unwrap(), b"fixture media contents");
    assert_eq!(fs::read(target).unwrap(), b"existing subtitle");
}

#[test]
fn cleanup_waits_for_main_and_every_enabled_short_output() {
    for (main_done, shorts_enabled, short_done, permitted) in [
        (false, false, false, false),
        (false, true, true, false),
        (true, true, false, false),
        (true, true, true, true),
        (true, false, false, true),
    ] {
        let mut f = Fixture::new();
        let path = f.owned("成片.mp4");
        f.task.main_done = main_done;
        f.task.short_done = short_done;
        f.task.config["firstEpisodeShorts"] = json!(shorts_enabled);
        let result = cleanup(&mut f.task);
        if permitted {
            result.unwrap();
            assert!(!path.exists());
        } else {
            assert_eq!(result.unwrap_err().code, "AUTOMATION_CLEANUP_BLOCKED");
            assert_eq!(fs::read(path).unwrap(), b"fixture media contents");
        }
    }
}

#[test]
fn cleanup_waits_for_recorded_short_work_even_if_setting_is_missing() {
    let mut f = Fixture::new();
    let path = f.owned("成片.mp4");
    f.task.short_merge_job = Some("short-merge-in-flight".into());
    let error = cleanup(&mut f.task).unwrap_err();
    assert_eq!(error.code, "AUTOMATION_CLEANUP_BLOCKED");
    assert!(
        path.exists(),
        "main media must remain while Shorts is pending"
    );
}

#[test]
fn cleanup_deletes_owned_media_but_preserves_untracked_media_and_receipts() {
    let mut f = Fixture::new();
    let episode = f.owned("0001_episode_auto.mp4");
    f.task.files.push(episode.clone());
    let final_path = f.owned("成片.mp4");
    let unrelated = f.file("用户保留.mp4", b"untracked user content");
    let record = f.file("自动追剧记录.json", br#"{"id":"fixture"}"#);
    let receipt = f.file("0001_episode_auto.receipt.json", br#"{"hash":"fixture"}"#);
    cleanup(&mut f.task).unwrap();
    assert!(!episode.exists());
    assert!(!final_path.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"untracked user content");
    assert_eq!(fs::read(record).unwrap(), br#"{"id":"fixture"}"#);
    assert_eq!(fs::read(receipt).unwrap(), br#"{"hash":"fixture"}"#);
}

#[test]
fn cleanup_flags_are_independent_for_episodes_finals_and_subtitles() {
    for delete_episodes in [false, true] {
        for delete_final in [false, true] {
            for keep_subtitles in [false, true] {
                let mut f = Fixture::new();
                f.task.config["deleteEpisodes"] = json!(delete_episodes);
                f.task.config["deleteFinal"] = json!(delete_final);
                f.task.config["keepSubtitles"] = json!(keep_subtitles);
                let episode = f.owned("0001_episode_auto.mp4");
                f.task.files.push(episode.clone());
                let short_source = f.owned("Shorts/首集源视频.mp4");
                let final_path = f.owned("成片.mp4");
                let short_final = f.owned("Shorts/成片.mp4");
                let srt = f.owned("字幕.srt");
                let vtt = f.owned("字幕.vtt");
                cleanup(&mut f.task).unwrap();
                for path in [episode, short_source] {
                    assert_eq!(path.exists(), !delete_episodes, "{}", path.display());
                }
                for path in [final_path, short_final] {
                    assert_eq!(path.exists(), !delete_final, "{}", path.display());
                }
                for path in [srt, vtt] {
                    assert_eq!(path.exists(), keep_subtitles, "{}", path.display());
                }
            }
        }
    }
}

#[test]
fn same_length_changed_content_is_preserved_and_blocks_cleanup() {
    let mut f = Fixture::new();
    let path = f.owned("成片.mp4");
    let original = identity(&path).unwrap();
    let replacement = vec![b'X'; original.size as usize];
    fs::write(&path, &replacement).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), original.size);
    assert_eq!(
        cleanup(&mut f.task).unwrap_err().code,
        "AUTOMATION_FILE_INVALID"
    );
    assert_eq!(fs::read(&path).unwrap(), replacement);
    assert_eq!(f.task.owned[0].hash, original.hash);
}

#[test]
fn resized_owned_content_is_preserved_and_blocks_cleanup() {
    let mut f = Fixture::new();
    let path = f.owned("成片.mp4");
    fs::write(&path, b"changed size").unwrap();
    assert_eq!(
        cleanup(&mut f.task).unwrap_err().code,
        "AUTOMATION_FILE_INVALID"
    );
    assert_eq!(fs::read(path).unwrap(), b"changed size");
}

#[test]
fn ownership_rejects_files_outside_the_specific_task_root() {
    let mut f = Fixture::new();
    let outside = f.base.join("另一频道成片.mp4");
    fs::write(&outside, b"other channel").unwrap();
    assert_eq!(
        own(&mut f.task, &outside).unwrap_err().code,
        "AUTOMATION_FILE_INVALID"
    );
    assert!(f.task.owned.is_empty());
    assert_eq!(fs::read(outside).unwrap(), b"other channel");
}

#[cfg(unix)]
#[test]
fn replaced_owned_path_cannot_follow_a_symlink_even_with_matching_contents() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let path = f.owned("成片.mp4");
    let outside = f.base.join("用户原件.mp4");
    fs::write(&outside, b"fixture media contents").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert_eq!(
        cleanup(&mut f.task).unwrap_err().code,
        "AUTOMATION_FILE_INVALID"
    );
    assert!(path.is_symlink());
    assert_eq!(fs::read(outside).unwrap(), b"fixture media contents");
}

#[cfg(unix)]
#[test]
fn ownership_rejects_symlinked_subdirectories_inside_task_root() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let outside_dir = f.base.join("其他任务");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("成片.mp4"), b"other task").unwrap();
    let linked_dir = f.task.root.join("Shorts");
    symlink(&outside_dir, &linked_dir).unwrap();
    assert_eq!(
        own(&mut f.task, &linked_dir.join("成片.mp4"))
            .unwrap_err()
            .code,
        "AUTOMATION_FILE_INVALID"
    );
    assert!(f.task.owned.is_empty());
}

#[test]
fn resumed_cleanup_is_idempotent_and_preserves_stage_handles_and_identity() {
    let mut f = Fixture::new();
    let episode = f.owned("0001_episode_auto.mp4");
    let final_path = f.owned("成片.mp4");
    f.task.files.push(episode.clone());
    f.task.merged = Some(final_path.clone());
    f.task.prepared = Some(final_path.clone());
    f.task.merge_job = Some("merge-fixture-id".into());
    f.task.separate_job = Some("separate-fixture-id".into());
    f.task.subtitle_job = Some("subtitle-fixture-id".into());
    f.task.short_merge_job = Some("short-merge-fixture-id".into());
    f.task.short_done = true;
    f.task.main_video_url = "https://www.youtube.com/watch?v=fixture".into();
    // Simulate process exit after deleting one file but before checkpointing.
    fs::remove_file(&episode).unwrap();
    let before = serde_json::to_value(&f.task).unwrap();
    cleanup(&mut f.task).unwrap();
    cleanup(&mut f.task).unwrap();
    assert!(!final_path.exists());
    assert_eq!(serde_json::to_value(&f.task).unwrap(), before);
}

#[test]
fn missing_derived_input_rebuilds_from_verified_originals_without_redownload() {
    let mut f = Fixture::new();
    f.task.main_done = false;
    f.task.stage = "separate".into();
    f.task.files = vec![f.owned("0001.mp4"), f.owned("0002.mp4")];
    f.task.episode_done = 2;
    f.task.episode_total = 2;
    let missing = f.task.root.join("合并视频/成片.mp4");
    f.task.merged = Some(missing.clone());
    f.task.prepared = Some(missing.clone());
    f.task.merge_job = Some("old-merge".into());
    f.task.separate_job = Some("old-separate".into());
    let files = f.task.files.clone();
    assert!(recover_missing_media_input(&mut f.task, &missing).unwrap());
    assert_eq!(f.task.stage, "merge");
    assert_eq!(f.task.files, files);
    assert!(f.task.merge_job.is_none());
    assert!(f.task.separate_job.is_none());
    assert!(f.task.message.contains("重建"));
}

#[test]
fn missing_vocals_reuses_verified_merged_video() {
    let mut f = Fixture::new();
    f.task.main_done = false;
    f.task.stage = "subtitles".into();
    f.task.config["separate"] = json!(true);
    f.task.merged = Some(f.owned("合并视频/成片.mp4"));
    let missing = f.task.root.join("音频分离/成片.mp4");
    f.task.prepared = Some(missing.clone());
    assert!(recover_missing_media_input(&mut f.task, &missing).unwrap());
    assert_eq!(f.task.stage, "separate");
    assert_eq!(f.task.prepared, f.task.merged);
}

#[test]
fn missing_media_recovery_rejects_changed_source_and_never_rewinds_uploaded_work() {
    let mut f = Fixture::new();
    f.task.main_done = false;
    f.task.stage = "subtitles".into();
    let original = f.owned("0001.mp4");
    f.task.files = vec![original.clone()];
    f.task.episode_done = 1;
    f.task.episode_total = 1;
    fs::write(original, b"changed").unwrap();
    let missing = f.task.root.join("missing.mp4");
    assert!(recover_missing_media_input(&mut f.task, &missing).is_err());
    assert_eq!(f.task.stage, "subtitles");
    f.task.main_done = true;
    assert!(!recover_missing_media_input(&mut f.task, &missing).unwrap());
    assert_eq!(f.task.stage, "subtitles");
}

#[test]
fn missing_completed_media_output_is_not_reused_but_active_work_is_preserved() {
    let f = Fixture::new();
    let path = f.task.root.join("missing.mp4");
    let mut job: MediaJob = serde_json::from_value(json!({
        "id":"old", "dedupeKey":"old", "kind":"merge", "status":"completed",
        "stage":"completed", "percent":100, "inputs":[], "outputPath":path
    }))
    .unwrap();
    assert!(!reusable_media(&job));
    job.status = MediaJobStatus::Running;
    assert!(reusable_media(&job));
    job.status = MediaJobStatus::Completed;
    fs::write(&path, b"output").unwrap();
    assert!(reusable_media(&job));
}

#[test]
fn a_scan_admits_only_free_slots_and_refills_each_completed_drama() {
    let f = Fixture::new();
    let config = json!({"channel":"fixture-channel", "concurrency":"3"});
    let candidates: Vec<_> = (0..150)
        .map(|i| {
            let mut c = f.task.source.clone();
            c.book_id = format!("book-{i}");
            c
        })
        .collect();
    let mut snapshot = Snapshot {
        config: Some(config.clone()),
        mode: Mode::Running,
        ..Default::default()
    };
    assert_eq!(
        admit_group(&mut snapshot, candidates.clone(), &config, &f.base),
        10
    );
    assert_eq!(snapshot.jobs.len(), 10);
    assert!(snapshot.waiting.is_empty());
    for job in snapshot.jobs.iter_mut().take(1) {
        job.status = Status::Completed;
    }
    assert_eq!(
        admit_group(&mut snapshot, candidates.clone(), &config, &f.base),
        1
    );
    assert_eq!(snapshot.jobs.len(), 11);
    assert_eq!(snapshot.jobs[10].book_id, "book-10");
    snapshot.jobs[9].status = Status::Completed;
    assert_eq!(admit_group(&mut snapshot, candidates, &config, &f.base), 1);
    assert_eq!(snapshot.jobs.len(), 12);
    assert_eq!(snapshot.jobs[10].book_id, "book-10");
    assert!(snapshot.waiting.is_empty());
}

#[test]
fn duration_limit_preserves_exactly_twelve_hours_and_skips_only_longer_unfinished_work() {
    let mut f = Fixture::new();
    f.task.main_done = false;
    f.task.stage = "separate".into();
    f.task.attempts = 19;
    f.task.retry_at = now() + 900;
    let file = f.owned("合并视频/超长.mp4");
    f.task.merged = Some(file.clone());
    assert!(!duration::skip(&mut f.task, 43200.0));
    assert!(duration::skip(&mut f.task, 43200.01));
    assert_eq!(f.task.status, Status::Skipped);
    assert_eq!(f.task.retry_at, 0);
    assert_eq!(f.task.attempts, 0);
    assert!(file.exists());
    assert_eq!(f.task.merged.as_ref(), Some(&file));
    f.task.main_done = true;
    f.task.status = Status::Pending;
    assert!(!duration::skip(&mut f.task, 48518.0));
    assert_eq!(f.task.status, Status::Pending);
}

#[test]
fn duration_cache_rechecks_replaced_media_and_preserves_missing_media_recovery() {
    let f = Fixture::new();
    let file = f.file("合并视频/全集.mp4", b"video");
    let cache = duration::inspect(&file, None, |_| Ok(31933.3)).unwrap();
    assert_eq!(
        duration::inspect(&file, cache.clone(), |_| panic!(
            "unchanged file must use cache"
        ))
        .unwrap()
        .unwrap()
        .seconds,
        31933.3
    );
    fs::write(&file, b"replacement-video").unwrap();
    assert_eq!(
        duration::inspect(&file, cache, |_| Ok(48518.0))
            .unwrap()
            .unwrap()
            .seconds,
        48518.0
    );
    fs::remove_file(&file).unwrap();
    assert!(duration::inspect(&file, None, |_| panic!(
        "missing file belongs to existing recovery flow"
    ))
    .unwrap()
    .is_none());
}

#[test]
fn merge_duration_preflight_stops_early_and_does_not_skip_unknown_durations() {
    let f = Fixture::new();
    let paths = vec![
        f.file("1.mp4", b"1"),
        f.file("2.mp4", b"2"),
        f.file("3.mp4", b"3"),
    ];
    let mut calls = 0;
    let total = duration::inputs_over_limit(&paths, |_| {
        calls += 1;
        Ok(25000.0)
    })
    .unwrap();
    assert_eq!(total, Some(50000.0));
    assert_eq!(calls, 2);
    assert_eq!(
        duration::inputs_over_limit(&paths, |_| Ok(14400.0)).unwrap(),
        None
    );
    for unknown in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        assert!(duration::inputs_over_limit(&paths, |_| Ok(unknown)).is_err());
    }
}

#[test]
fn skipped_task_guard_blocks_late_side_effects_while_other_tasks_keep_running() {
    let f = Fixture::new();
    let service = Service::load(f.base.join("guard-state.json")).unwrap();
    service
        .transaction(|s| {
            s.mode = Mode::Running;
            s.jobs.push(f.task.clone());
            Ok(())
        })
        .unwrap();
    assert!(ensure_task_running(&service, &f.task.id).is_ok());
    service.review(&f.task.id, "skip").unwrap();
    assert_eq!(
        ensure_task_running(&service, &f.task.id).unwrap_err().code,
        "AUTOMATION_SKIPPED"
    );
    assert!(service.running());
}
