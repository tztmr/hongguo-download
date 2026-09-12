//! Delete only task-owned outputs, never inputs or an entire series directory.
use super::model::{MediaJob, MediaJobKind, MediaJobOutputKind, MediaJobStatus, MergeInput};
use crate::AppError;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

fn deletion_error(detail: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_OUTPUT_DELETE_FAILED",
        "无法删除任务产物，请检查文件占用与路径",
        detail.to_string(),
    )
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn validate_path(root: &Path, path: &Path) -> Result<(), AppError> {
    if !path.is_absolute()
        || path == root
        || !path.starts_with(root)
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(deletion_error("output is outside its task directory"));
    }
    // Check every existing ancestor, including the series and drive path.
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        // A drive prefix alone (especially \\?\C:) is not a filesystem object.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if is_link(&meta) => {
                return Err(deletion_error("output path contains a link/reparse point"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(deletion_error(error)),
        }
    }
    Ok(())
}

pub(crate) fn output_paths(job: &MediaJob) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = BTreeSet::new();
    if job.kind == MediaJobKind::Merge {
        paths.extend(job.output_path.iter().cloned());
    } else {
        for output in &job.outputs {
            let owned = match job.kind {
                MediaJobKind::SeparateBackgroundMusic => matches!(
                    output.kind,
                    MediaJobOutputKind::Vocals
                        | MediaJobOutputKind::BackgroundMusic
                        | MediaJobOutputKind::NoBackgroundMusicVideo
                ),
                MediaJobKind::ExtractSubtitles => output.kind == MediaJobOutputKind::Subtitles,
                _ => false,
            };
            if owned {
                paths.insert(output.path.clone());
            }
        }
    }
    let series = job
        .merge_request
        .as_ref()
        .map(|r| &r.series_root)
        .or_else(|| job.ai_request.as_ref().map(|r| &r.series_root));
    let Some(series) = series else {
        return if paths.is_empty() {
            Ok(Vec::new())
        } else {
            Err(deletion_error("missing validated output root"))
        };
    };
    let family = match job.kind {
        MediaJobKind::Merge => "合并视频",
        MediaJobKind::SeparateBackgroundMusic => "音频分离",
        MediaJobKind::ExtractSubtitles => "字幕",
    };
    let root = series.join(family);
    // An interrupted multi-episode worker may have published completed episodes
    // before its final outputs reached the job record. Find only matching
    // completion records, not arbitrary files in the family folder.
    if let Some(request) = &job.ai_request {
        let scopes = match fs::read_dir(&root) {
            Ok(scopes) => Some(scopes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(deletion_error(error)),
        };
        if let Some(scopes) = scopes {
            for scope in scopes {
                let scope = scope.map_err(deletion_error)?;
                validate_path(&root, &scope.path())?;
                if !scope.file_type().map_err(deletion_error)?.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(scope.path()).map_err(deletion_error)? {
                    let directory = entry.map_err(deletion_error)?.path();
                    validate_path(&root, &directory)?;
                    let marker = directory.join("result.json");
                    validate_path(&root, &marker)?;
                    let Ok(meta) = fs::metadata(&marker) else {
                        continue;
                    };
                    if !meta.is_file() || meta.len() > 1024 * 1024 {
                        continue;
                    }
                    let Ok(record) = serde_json::from_slice::<serde_json::Value>(
                        &fs::read(&marker).map_err(deletion_error)?,
                    ) else {
                        continue;
                    };
                    let identity = &record["identity"];
                    let input =
                        serde_json::from_value::<MergeInput>(identity["source"]["input"].clone());
                    if identity["kind"] != serde_json::to_value(job.kind).unwrap()
                        || identity["model"].as_str() != Some(request.model.as_str())
                        || identity["source"]["book_id"].as_str() != Some(request.book_id.as_str())
                        || identity["source"]["scope"]
                            != serde_json::to_value(request.scope).unwrap()
                        || !input.is_ok_and(|input| request.inputs.contains(&input))
                    {
                        continue;
                    }
                    if let Some(files) = record["files"].as_array() {
                        for file in files {
                            let Some(name) = file["name"].as_str() else {
                                continue;
                            };
                            let allowed = if job.kind == MediaJobKind::SeparateBackgroundMusic {
                                ["人声.wav", "背景音乐.wav", "去背景音乐.mp4"].contains(&name)
                            } else {
                                name == "字幕.srt"
                            };
                            if allowed {
                                paths.insert(directory.join(name));
                            }
                        }
                    }
                    paths.insert(marker);
                }
            }
        }
    }
    for path in &paths {
        validate_path(&root, path)?;
        if job.inputs.iter().any(|input| input.path == *path) {
            return Err(deletion_error("refusing to delete an input file"));
        }
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => {
                return Err(deletion_error("output is not a regular file"))
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(deletion_error(error))
            }
            _ => {}
        }
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    // Keep completion records until all media files are removed so a retry can
    // still locate results after a partially successful deletion.
    paths.sort_by_key(|path| path.file_name().is_some_and(|name| name == "result.json"));
    Ok(paths)
}

pub(crate) fn remove_outputs(job: &MediaJob, jobs: &[MediaJob]) -> Result<(), AppError> {
    let paths = output_paths(job)?;
    if jobs.iter().any(|other| {
        other.id != job.id
            && matches!(
                other.status,
                MediaJobStatus::Queued | MediaJobStatus::Running | MediaJobStatus::Paused
            )
            && other.inputs.iter().any(|input| paths.contains(&input.path))
    }) {
        return Err(AppError::new(
            "MEDIA_OUTPUT_IN_USE",
            "产物正被其他媒体任务使用，请先停止相关任务",
        ));
    }
    let series = job
        .merge_request
        .as_ref()
        .map(|r| &r.series_root)
        .or_else(|| job.ai_request.as_ref().map(|r| &r.series_root));
    for path in &paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(deletion_error(error)),
        }
    }
    // Empty directories only: unrelated files and other task results survive.
    for path in &paths {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if Some(directory) == series.map(PathBuf::as_path) {
                break;
            }
            if fs::remove_dir(directory).is_err() {
                break;
            }
            parent = directory.parent();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture(kind: &str) -> (PathBuf, MediaJob) {
        let root = std::env::temp_dir().join(format!(
            "hongguo-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let input = root.join("original.mp4");
        fs::write(&input, b"original").unwrap();
        let snapshot = json!({"episodeIndex":1,"path":input,"size":8,"modifiedUnixNanos":0});
        let mut job: MediaJob = serde_json::from_value(json!({
            "id":"delete", "dedupeKey":"delete", "kind":kind, "status":"completed", "stage":"completed", "percent":100,
            "inputs":[{"path":input,"sizeBytes":8}], "outputPath":null,"errorCode":null,"errorMessage":null,
            "aiRequest":{"bookId":"book","title":"title","kind":kind,"scope":"merged","seriesRoot":root,
                "inputs":[snapshot],"model":"model","device":"auto","dedupeKey":"delete"}
        })).unwrap();
        if kind == "merge" {
            job.merge_request = Some(serde_json::from_value(json!({
                "bookId":"book","title":"title","scope":"merged","seriesRoot":root,"outputFileName":"merged.mp4",
                "inputs":[snapshot],"conflictPolicy":"failIfExists","dedupeKey":"delete"
            })).unwrap());
            job.ai_request = None;
        }
        (root, job)
    }
    fn write(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"output").unwrap();
        path
    }
    #[test]
    fn deletes_only_merge_output_and_empty_directory() {
        let (root, mut job) = fixture("merge");
        let output = write(&root, "合并视频/merged.mp4");
        let music = write(&root, "音频分离/背景音乐.wav");
        job.output_path = Some(output.clone());
        remove_outputs(&job, &[]).unwrap();
        assert!(!output.exists());
        assert!(!root.join("合并视频").exists());
        assert!(music.exists());
        assert!(job.inputs[0].path.exists());
        remove_outputs(&job, &[]).unwrap();
    }
    #[test]
    fn separation_deletes_its_outputs_but_keeps_merge_subtitles_and_unrelated_files() {
        let (root, mut job) = fixture("separateBackgroundMusic");
        for (name, kind) in [
            ("人声.wav", MediaJobOutputKind::Vocals),
            ("背景音乐.wav", MediaJobOutputKind::BackgroundMusic),
            ("去背景音乐.mp4", MediaJobOutputKind::NoBackgroundMusicVideo),
        ] {
            job.outputs.push(super::super::model::MediaJobOutput {
                episode_index: 1,
                kind,
                path: write(&root, &format!("音频分离/整季/result/{name}")),
                source: None,
            });
        }
        let merge = write(&root, "合并视频/merged.mp4");
        let subtitles = write(&root, "字幕/subtitles.srt");
        let unrelated = write(&root, "音频分离/整季/result/notes.txt");
        remove_outputs(&job, &[]).unwrap();
        assert!(job.outputs.iter().all(|output| !output.path.exists()));
        assert!(
            merge.exists()
                && subtitles.exists()
                && unrelated.exists()
                && job.inputs[0].path.exists()
        );
    }
    #[test]
    fn rejects_outside_directory_and_busy_outputs_without_deleting() {
        let (root, mut job) = fixture("merge");
        job.output_path = Some(job.inputs[0].path.clone());
        assert!(remove_outputs(&job, &[]).is_err());
        let output = write(&root, "合并视频/merged.mp4");
        job.output_path = Some(output.clone());
        let mut dependent = job.clone();
        dependent.id = "dependent".into();
        dependent.status = MediaJobStatus::Paused;
        dependent.inputs[0].path = output.clone();
        assert_eq!(
            remove_outputs(&job, &[dependent]).unwrap_err().code,
            "MEDIA_OUTPUT_IN_USE"
        );
        assert!(output.exists() && job.inputs[0].path.exists());
    }
    #[test]
    fn interrupted_job_finds_only_matching_completion_records() {
        let (root, job) = fixture("extractSubtitles");
        let output = write(&root, "字幕/整季/result/字幕.srt");
        let marker = output.parent().unwrap().join("result.json");
        let request = job.ai_request.as_ref().unwrap();
        fs::write(
            &marker,
            serde_json::to_vec(&json!({"identity":{"kind":job.kind,"model":request.model,
            "source":{"book_id":request.book_id,"scope":request.scope,"input":request.inputs[0]}},
            "files":[{"name":"字幕.srt"}]}))
            .unwrap(),
        )
        .unwrap();
        let unrelated = write(&root, "字幕/整季/other/字幕.srt");
        assert_eq!(output_paths(&job).unwrap().last().unwrap(), &marker);
        remove_outputs(&job, &[]).unwrap();
        assert!(!output.exists() && !marker.exists());
        assert!(unrelated.exists());
    }
}
