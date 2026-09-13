use super::super::source::Candidate;
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    base: PathBuf,
    task: Task,
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
    f.task.main_video_url = "https://www.youtube.com/watch?v=fixture".into();
    // Simulate process exit after deleting one file but before checkpointing.
    fs::remove_file(&episode).unwrap();
    let before = serde_json::to_value(&f.task).unwrap();
    cleanup(&mut f.task).unwrap();
    cleanup(&mut f.task).unwrap();
    assert!(!final_path.exists());
    assert_eq!(serde_json::to_value(&f.task).unwrap(), before);
}
