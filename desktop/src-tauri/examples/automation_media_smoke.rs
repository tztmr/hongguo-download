//! Local-only smoke: native main/Shorts merges, validation, and durable queue recovery.
//! Usage: automation_media_smoke <new-output-directory> <episode-fixture.mp4>
use hongguo_desktop_lib::{
    media::{
        model::MergeMode,
        MediaJob, MediaJobEventSink, MediaJobManager, MediaJobService, MediaJobStatus,
        NativeMergeExecutor, StartMergeInput, StartMergeRequest,
    },
    youtube::format::{self, UploadFormat},
};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
struct Sink;
impl MediaJobEventSink for Sink {
    fn emit(&self, _: MediaJob) {}
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).ok_or("output directory required")?);
    fs::create_dir(&root)?;
    let fixture = PathBuf::from(args.get(2).ok_or("local episode fixture required")?);
    let path = root.join("media-state");
    let manager = Arc::new(MediaJobManager::load(&path)?);
    let service = MediaJobService::new(
        manager,
        Arc::new(NativeMergeExecutor::from_packaged_tools()),
        Arc::new(Sink),
    );
    let mut reports = vec![];
    for short in [false, true] {
        let dir = root.join(if short { "Shorts" } else { "正片" });
        fs::create_dir(&dir)?;
        let mut inputs = vec![];
        for index in 1..=if short { 1 } else { 2 } {
            let path = dir.join(format!("{index:04}_fixture-id.mp4"));
            fs::copy(&fixture, &path)?;
            inputs.push(StartMergeInput {
                episode_index: index,
                path,
            });
        }
        let request = StartMergeRequest {
            book_id: format!("auto-smoke-{}", if short { "short-merge" } else { "merge" }),
            title: "自动追剧本地验证第二季".into(),
            series_root: dir,
            output_file_name: "成片.mp4".into(),
            inputs,
            transcode_h264: false,
            mode: Some(MergeMode::Auto),
            quality: Default::default(),
            conflict_policy: Default::default(),
        };
        let job = service.start_merge(request)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let finished = loop {
            let job = service
                .snapshot()
                .jobs
                .into_iter()
                .find(|j| j.id == job.id)
                .unwrap();
            if matches!(
                job.status,
                MediaJobStatus::Failed | MediaJobStatus::Cancelled
            ) {
                return Err(format!("merge failed: {:?}", job.error_message).into());
            }
            if job.status == MediaJobStatus::Completed {
                break job;
            }
            if Instant::now() > deadline {
                return Err("merge timeout".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let output = finished
            .output_path
            .as_ref()
            .ok_or("missing merge output")?;
        assert!(service.is_validated_upload_source(output));
        assert_eq!(format::orientation_file(output)?, "vertical");
        format::validate_file(
            if short {
                UploadFormat::Shorts
            } else {
                UploadFormat::Standard
            },
            output,
        )?;
        reports.push(
            serde_json::json!({"kind":if short {"shorts"} else {"main"},"jobId":finished.id,
            "output":output,"nativeMerge":"completed","validatedUploadSource":true}),
        );
    }
    drop(service);
    let restored = MediaJobManager::load(&path)?.snapshot();
    assert_eq!(restored.jobs.len(), 2);
    assert!(restored
        .jobs
        .iter()
        .all(|j| j.status == MediaJobStatus::Completed));
    let report = serde_json::json!({"networkRequests":0,"uploads":0,"recoveredCompletedJobs":2,"results":reports});
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}
