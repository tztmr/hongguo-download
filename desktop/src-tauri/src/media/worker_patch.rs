//! Optional, hash-matched local worker update. The installed base stays intact.
use crate::AppError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

static PATCH_LOCK: Mutex<()> = Mutex::new(());
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Patch {
    base_sha256: String,
    worker_sha256: String,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn error(detail: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "AI_WORKER_UPDATE_FAILED",
        "AI 工作程序更新失败，请检查组件目录权限",
        detail.to_string(),
    )
}
fn regular_file(path: &Path) -> Result<Vec<u8>, AppError> {
    let meta = fs::symlink_metadata(path).map_err(error)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(error("worker is a reparse point"));
        }
    }
    if !meta.is_file() || meta.len() > 128 * 1024 * 1024 {
        return Err(error("invalid worker file"));
    }
    fs::read(path).map_err(error)
}

pub(super) fn resolve(base: &Path, bundle: &Path) -> Result<PathBuf, AppError> {
    let patch_root = bundle.join("resources/ai-worker-modern-v034");
    let manifest = patch_root.join("patch.json");
    if !manifest.try_exists().map_err(error)? {
        return Ok(base.to_path_buf());
    }
    let patch: Patch = serde_json::from_slice(&regular_file(&manifest)?).map_err(error)?;
    // Different runtime versions/flavors retain their own dependency-compatible worker.
    if digest(&regular_file(base)?) != patch.base_sha256 {
        return Ok(base.to_path_buf());
    }
    if patch.worker_sha256.len() != 64
        || !patch.worker_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(error("invalid worker checksum"));
    }
    let _guard = PATCH_LOCK
        .lock()
        .map_err(|_| error("worker update lock unavailable"))?;
    let parent = base
        .parent()
        .ok_or_else(|| error("missing worker directory"))?;
    let target = parent.join(format!(
        "hongguo-ai-worker-{}.exe",
        &patch.worker_sha256[..16]
    ));
    if target.try_exists().map_err(error)? {
        return if digest(&regular_file(&target)?) == patch.worker_sha256 {
            Ok(target)
        } else {
            Err(error("installed worker update checksum mismatch"))
        };
    }
    let bytes = regular_file(&patch_root.join("hongguo-ai-worker.exe"))?;
    if digest(&bytes) != patch.worker_sha256 {
        return Err(error("packaged worker checksum mismatch"));
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let temporary = parent.join(format!(".worker-update-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(error)?;
        file.write_all(&bytes).map_err(error)?;
        file.sync_all().map_err(error)?;
        drop(file);
        fs::rename(&temporary, &target).map_err(error)?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applies_only_matching_base_and_preserves_original() {
        let root = std::env::temp_dir().join(format!(
            "hongguo-worker-patch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let patch = root.join("resources/ai-worker-modern-v034");
        fs::create_dir_all(&patch).unwrap();
        let base = root.join("worker.exe");
        fs::write(&base, b"base").unwrap();
        assert_eq!(resolve(&base, &root).unwrap(), base);
        fs::write(patch.join("hongguo-ai-worker.exe"), b"updated").unwrap();
        fs::write(patch.join("patch.json"),serde_json::to_vec(&serde_json::json!({"baseSha256":digest(b"base"),"workerSha256":digest(b"updated")})).unwrap()).unwrap();
        let updated = resolve(&base, &root).unwrap();
        assert_ne!(updated, base);
        assert_eq!(fs::read(&updated).unwrap(), b"updated");
        assert_eq!(fs::read(&base).unwrap(), b"base");
        assert_eq!(resolve(&base, &root).unwrap(), updated);
        fs::write(&base, b"another-version").unwrap();
        assert_eq!(resolve(&base, &root).unwrap(), base);
        fs::write(&base, b"base").unwrap();
        fs::write(&updated, b"corrupt").unwrap();
        assert!(resolve(&base, &root).is_err());
    }
}
