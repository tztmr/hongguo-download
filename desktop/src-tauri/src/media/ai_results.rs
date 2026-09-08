//! Immutable, completed AI result directories. A directory becomes reusable only
//! when all files and their completion record are published together by rename.
use super::{ai_io, regular_nonempty, safe_output_directory, JobTemp};
use crate::{
    media::{CancellationToken, MediaJobKind, MediaJobScope, MergeInput, ValidatedAIJobRequest},
    AppError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const RECORD: &str = "result.json";
pub(super) const VOCALS: &str = "人声.wav";
pub(super) const MUSIC: &str = "背景音乐.wav";
pub(super) const VIDEO: &str = "去背景音乐.mp4";
pub(super) const SUBTITLES: &str = "字幕.srt";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SourceIdentity {
    book_id: String,
    scope: MediaJobScope,
    input: MergeInput,
}

impl SourceIdentity {
    fn verify(&self) -> Result<(), AppError> {
        let path = fs::canonicalize(&self.input.path).map_err(ai_io)?;
        let metadata = fs::metadata(&path).map_err(ai_io)?;
        if path != self.input.path
            || !metadata.is_file()
            || metadata.len() != self.input.size
            || crate::media::metadata_modified_unix_nanos(&metadata)?
                != self.input.modified_unix_nanos
        {
            return Err(AppError::new(
                "MEDIA_INPUT_CHANGED",
                "源视频已变化，请重新创建 AI 任务",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    version: u32,
    source: SourceIdentity,
    kind: MediaJobKind,
    model: String,
    device: String,
    runtime: PathBuf,
    model_root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResultFile {
    name: String,
    size: u64,
    modified_unix_nanos: u128,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionRecord {
    identity: Identity,
    files: Vec<ResultFile>,
    subtitle_source: Option<String>,
}

pub(super) struct PublishedResult {
    root: PathBuf,
    pub subtitle_source: Option<String>,
}

impl PublishedResult {
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

pub(super) struct ResultStore {
    series_root: PathBuf,
    parent: PathBuf,
    prefix: String,
    identity: Identity,
}

fn scope_name(scope: MediaJobScope) -> &'static str {
    match scope {
        MediaJobScope::Merged => "整季",
        MediaJobScope::Episodes => "逐集",
    }
}

fn expected_files(kind: MediaJobKind) -> &'static [&'static str] {
    match kind {
        MediaJobKind::SeparateBackgroundMusic => &[VOCALS, MUSIC, VIDEO],
        MediaJobKind::ExtractSubtitles => &[SUBTITLES],
        MediaJobKind::Merge => &[],
    }
}

pub(super) fn nonce() -> Result<String, AppError> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(ai_io)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl ResultStore {
    pub fn new(
        request: &ValidatedAIJobRequest,
        episode: u32,
        input: &Path,
        runtime: &Path,
        model_root: &Path,
    ) -> Result<Self, AppError> {
        let snapshot = request
            .inputs
            .iter()
            .find(|item| item.episode_index == episode && item.path == input)
            .ok_or_else(|| AppError::new("AI_INPUT_INVALID", "AI 任务缺少源视频快照"))?;
        let identity = Identity {
            version: 1,
            source: SourceIdentity {
                book_id: request.book_id.clone(),
                scope: request.scope,
                input: snapshot.clone(),
            },
            kind: request.kind,
            model: request.model.clone(),
            device: request.device.clone(),
            runtime: runtime.to_path_buf(),
            model_root: model_root.to_path_buf(),
        };
        identity.source.verify()?;
        let family = match request.kind {
            MediaJobKind::SeparateBackgroundMusic => "音频分离",
            MediaJobKind::ExtractSubtitles => "字幕",
            MediaJobKind::Merge => {
                return Err(AppError::new("AI_REQUEST_INVALID", "AI 任务类型无效"))
            }
        };
        let root = safe_output_directory(&request.series_root, family)?;
        let parent = safe_output_directory(&root, scope_name(request.scope))?;
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&identity).map_err(ai_io)?)
        );
        Ok(Self {
            series_root: request.series_root.clone(),
            parent,
            prefix: format!("{episode:03}_{}_{}", request.model, &hash[..16]),
            identity,
        })
    }

    pub fn lookup(&self) -> Option<PublishedResult> {
        self.identity.source.verify().ok()?;
        completed_directories(&self.parent)
            .into_iter()
            .find_map(|root| {
                if !root
                    .file_name()?
                    .to_str()?
                    .starts_with(&format!("{}-", self.prefix))
                {
                    return None;
                }
                let record = read_completed(&root)?;
                (record.identity == self.identity).then_some(PublishedResult {
                    root,
                    subtitle_source: record.subtitle_source,
                })
            })
    }

    pub fn find_vocals(&self) -> Option<PathBuf> {
        self.identity.source.verify().ok()?;
        let parent = self
            .series_root
            .join("音频分离")
            .join(scope_name(self.identity.source.scope));
        // Legacy flat files carry no source identity and cannot safely be reused.
        completed_directories(&parent).into_iter().find_map(|root| {
            if !root
                .file_name()?
                .to_str()?
                .starts_with(&format!("{:03}_", self.identity.source.input.episode_index))
            {
                return None;
            }
            let record = read_completed(&root)?;
            (record.identity.kind == MediaJobKind::SeparateBackgroundMusic
                && record.identity.source == self.identity.source)
                .then(|| root.join(VOCALS))
        })
    }

    pub fn publish(
        &self,
        temp: &JobTemp,
        files: &[(&str, &Path)],
        subtitle_source: Option<String>,
        cancellation: &CancellationToken,
    ) -> Result<PublishedResult, AppError> {
        self.identity.source.verify()?;
        if files.iter().map(|(name, _)| *name).collect::<Vec<_>>()
            != expected_files(self.identity.kind)
        {
            return Err(AppError::new("AI_OUTPUT_INVALID", "AI 结果文件不完整"));
        }
        let staging = temp.root.join("publish");
        fs::create_dir(&staging).map_err(ai_io)?;
        let mut recorded = Vec::new();
        for (name, source) in files {
            if cancellation.is_cancelled() {
                return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
            }
            if !regular_nonempty(source) {
                return Err(AppError::new("AI_OUTPUT_INVALID", "AI 结果文件无效"));
            }
            let destination = staging.join(name);
            // Both paths are inside this series' filesystem. Never copy directly
            // into a public result path, including when a move fails.
            fs::rename(source, &destination).map_err(ai_io)?;
            let metadata = fs::metadata(&destination).map_err(ai_io)?;
            recorded.push(ResultFile {
                name: (*name).into(),
                size: metadata.len(),
                modified_unix_nanos: crate::media::metadata_modified_unix_nanos(&metadata)?,
            });
        }
        let record = CompletionRecord {
            identity: self.identity.clone(),
            files: recorded,
            subtitle_source: subtitle_source.clone(),
        };
        let mut marker = fs::File::create(staging.join(RECORD)).map_err(ai_io)?;
        marker
            .write_all(&serde_json::to_vec(&record).map_err(ai_io)?)
            .map_err(ai_io)?;
        marker.sync_all().map_err(ai_io)?;
        drop(marker);
        self.identity.source.verify()?;
        if cancellation.is_cancelled() {
            return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
        }
        // A new immutable generation also recovers from a damaged older result,
        // without deleting files referenced by previous completed tasks.
        let root = self.parent.join(format!("{}-{}", self.prefix, nonce()?));
        fs::rename(&staging, &root).map_err(ai_io)?;
        Ok(PublishedResult {
            root,
            subtitle_source,
        })
    }
}

fn completed_directories(parent: &Path) -> Vec<PathBuf> {
    let Ok(metadata) = fs::symlink_metadata(parent) else {
        return Vec::new();
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Vec::new();
    }
    let mut directories = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    directories.sort();
    directories
}

fn read_completed(root: &Path) -> Option<CompletionRecord> {
    let path = root.join(RECORD);
    if !regular_nonempty(&path) || fs::metadata(&path).ok()?.len() > 64 * 1024 {
        return None;
    }
    let record: CompletionRecord = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if record.identity.version != 1
        || record
            .files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>()
            != expected_files(record.identity.kind)
        || record.files.is_empty()
    {
        return None;
    }
    for file in &record.files {
        let path = root.join(&file.name);
        if !regular_nonempty(&path) {
            return None;
        }
        let metadata = fs::metadata(path).ok()?;
        if metadata.len() != file.size
            || crate::media::metadata_modified_unix_nanos(&metadata).ok()?
                != file.modified_unix_nanos
        {
            return None;
        }
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{validate_ai_request, StartAIJobRequest, StartMergeInput};

    struct Fixture {
        temp: JobTemp,
        input: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = JobTemp::create(&std::env::temp_dir()).unwrap();
            let input = temp.root.join("中文 源视频.mp4");
            fs::write(&input, b"original video").unwrap();
            Self { temp, input }
        }

        fn request(
            &self,
            scope: MediaJobScope,
            kind: MediaJobKind,
            model: &str,
        ) -> ValidatedAIJobRequest {
            validate_ai_request(
                &StartAIJobRequest {
                    book_id: "book".into(),
                    title: "剧目".into(),
                    series_root: self.temp.root.clone(),
                    scope,
                    inputs: vec![StartMergeInput {
                        episode_index: 1,
                        path: self.input.clone(),
                    }],
                    model: model.into(),
                    device: "cpu".into(),
                },
                kind,
            )
            .unwrap()
        }

        fn store(&self, scope: MediaJobScope, kind: MediaJobKind, model: &str) -> ResultStore {
            let request = self.request(scope, kind, model);
            self.for_request(&request)
        }

        fn for_request(&self, request: &ValidatedAIJobRequest) -> ResultStore {
            ResultStore::new(
                request,
                1,
                &request.inputs[0].path,
                &self.temp.root.join("runtime/2/worker"),
                &self.temp.root.join("models/1"),
            )
            .unwrap()
        }

        fn publish(&self, store: &ResultStore) -> PublishedResult {
            let temp = JobTemp::create(&self.temp.root).unwrap();
            let files = self.files(&temp, store.identity.kind);
            store
                .publish(
                    &temp,
                    &files
                        .iter()
                        .map(|(name, path)| (*name, path.as_path()))
                        .collect::<Vec<_>>(),
                    Some("whisperOriginalAudio".into()),
                    &CancellationToken::default(),
                )
                .unwrap()
        }

        fn files(&self, temp: &JobTemp, kind: MediaJobKind) -> Vec<(&'static str, PathBuf)> {
            expected_files(kind)
                .iter()
                .map(|name| {
                    let path = temp.output.join(name);
                    fs::write(&path, format!("validated output: {name}")).unwrap();
                    (*name, path)
                })
                .collect()
        }
    }

    #[test]
    fn scope_and_source_identity_isolate_separation_and_vocal_reuse() {
        let fixture = Fixture::new();
        let episodes = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let first = fixture.publish(&episodes);
        let merged = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        assert!(merged.lookup().is_none());
        let episode_subs = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::ExtractSubtitles,
            "small",
        );
        let merged_subs = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::ExtractSubtitles,
            "small",
        );
        assert_eq!(episode_subs.find_vocals(), Some(first.path(VOCALS)));
        assert!(merged_subs.find_vocals().is_none());
        let whole = fixture.publish(&merged);
        assert_ne!(first.path(VIDEO), whole.path(VIDEO));
        assert_eq!(merged_subs.find_vocals(), Some(whole.path(VOCALS)));
        fs::write(&fixture.input, b"replaced video with different contents").unwrap();
        let changed = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::ExtractSubtitles,
            "small",
        );
        assert!(changed.find_vocals().is_none());
        assert!(merged.lookup().is_none());
    }

    #[test]
    fn cache_matches_model_runtime_and_each_input_not_the_entire_batch() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let result = fixture.publish(&store);
        assert_eq!(store.lookup().unwrap().path(VIDEO), result.path(VIDEO));
        let other_model = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs_ft",
        );
        assert!(other_model.lookup().is_none());
        let request = fixture.request(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let newer_runtime = ResultStore::new(
            &request,
            1,
            &request.inputs[0].path,
            &fixture.temp.root.join("runtime/3/worker"),
            &fixture.temp.root.join("models/1"),
        )
        .unwrap();
        assert!(newer_runtime.lookup().is_none());
        let mut bigger_batch = request;
        let mut next = bigger_batch.inputs[0].clone();
        next.episode_index = 2;
        bigger_batch.inputs.push(next);
        assert_eq!(
            fixture
                .for_request(&bigger_batch)
                .lookup()
                .unwrap()
                .path(VIDEO),
            result.path(VIDEO)
        );
    }

    #[test]
    fn changed_source_is_rejected_before_publishing() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let temp = JobTemp::create(&fixture.temp.root).unwrap();
        let files = fixture.files(&temp, MediaJobKind::SeparateBackgroundMusic);
        fs::write(&fixture.input, b"changed source with a different length").unwrap();
        let error = store
            .publish(
                &temp,
                &files
                    .iter()
                    .map(|(n, p)| (*n, p.as_path()))
                    .collect::<Vec<_>>(),
                None,
                &CancellationToken::default(),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, "MEDIA_INPUT_CHANGED");
        assert!(completed_directories(&store.parent).is_empty());
    }

    #[test]
    fn failure_after_first_staged_file_leaves_no_public_partial_and_retry_succeeds() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let interrupted = JobTemp::create(&fixture.temp.root).unwrap();
        let files = fixture.files(&interrupted, MediaJobKind::SeparateBackgroundMusic);
        fs::remove_file(&files[1].1).unwrap();
        assert!(store
            .publish(
                &interrupted,
                &files
                    .iter()
                    .map(|(n, p)| (*n, p.as_path()))
                    .collect::<Vec<_>>(),
                None,
                &CancellationToken::default()
            )
            .is_err());
        assert!(interrupted.root.join("publish").join(VOCALS).exists());
        assert!(completed_directories(&store.parent).is_empty());
        assert!(store.lookup().is_none());
        // Leave the interrupted staging directory in place, as after a crash.
        let result = fixture.publish(&store);
        assert_eq!(store.lookup().unwrap().path(VIDEO), result.path(VIDEO));
        assert!(result.path(VOCALS).exists() && result.path(MUSIC).exists());
    }

    #[test]
    fn incomplete_or_damaged_bundle_is_rebuilt_without_overwriting_old_files() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let first = fixture.publish(&store);
        fs::write(first.path(VOCALS), b"damaged").unwrap();
        assert!(store.lookup().is_none());
        let second = fixture.publish(&store);
        assert_ne!(first.root, second.root);
        assert_eq!(fs::read(first.path(VOCALS)).unwrap(), b"damaged");
        assert_eq!(store.lookup().unwrap().root, second.root);
        fs::remove_file(second.root.join(RECORD)).unwrap();
        assert!(store.lookup().is_none());
        let third = fixture.publish(&store);
        assert_eq!(store.lookup().unwrap().root, third.root);
    }

    #[test]
    fn subtitles_are_scoped_and_preserve_their_recorded_source() {
        let fixture = Fixture::new();
        let episodes = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::ExtractSubtitles,
            "small",
        );
        let first = fixture.publish(&episodes);
        assert_eq!(
            episodes.lookup().unwrap().subtitle_source.as_deref(),
            Some("whisperOriginalAudio")
        );
        let merged = fixture.store(
            MediaJobScope::Merged,
            MediaJobKind::ExtractSubtitles,
            "small",
        );
        assert!(merged.lookup().is_none());
        assert_ne!(
            fixture.publish(&merged).path(SUBTITLES),
            first.path(SUBTITLES)
        );
    }

    #[test]
    fn legacy_flat_files_are_preserved_and_never_used_as_identified_results() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let old = fixture.temp.root.join("音频分离/001_htdemucs_人声.wav");
        fs::write(&old, b"legacy user file").unwrap();
        assert!(store.lookup().is_none());
        assert!(store.find_vocals().is_none());
        fixture.publish(&store);
        assert_eq!(fs::read(old).unwrap(), b"legacy user file");
    }

    #[test]
    fn cancellation_does_not_commit_an_incomplete_result() {
        let fixture = Fixture::new();
        let store = fixture.store(
            MediaJobScope::Episodes,
            MediaJobKind::SeparateBackgroundMusic,
            "htdemucs",
        );
        let temp = JobTemp::create(&fixture.temp.root).unwrap();
        let files = fixture.files(&temp, MediaJobKind::SeparateBackgroundMusic);
        let token = CancellationToken::default();
        token.cancel();
        let error = store
            .publish(
                &temp,
                &files
                    .iter()
                    .map(|(n, p)| (*n, p.as_path()))
                    .collect::<Vec<_>>(),
                None,
                &token,
            )
            .err()
            .unwrap();
        assert_eq!(error.code, "AI_CANCELLED");
        assert!(store.lookup().is_none());
        assert!(completed_directories(&store.parent).is_empty());
    }
}
