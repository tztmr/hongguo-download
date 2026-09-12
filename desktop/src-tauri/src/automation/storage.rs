use super::model::*;
use crate::AppError;
use atomicwrites::{AllowOverwrite, AtomicFile};
use std::{fs, io::Write, path::Path};

pub fn load(path: &Path) -> Result<Snapshot, AppError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            AppError::new(
                "AUTOMATION_STATE_INVALID",
                "自动追剧记录损坏，请保留文件并恢复备份",
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot::default()),
        Err(_) => Err(error()),
    }
}
pub fn save(path: &Path, value: &Snapshot) -> Result<(), AppError> {
    fs::create_dir_all(path.parent().ok_or_else(error)?).map_err(|_| error())?;
    let bytes = serde_json::to_vec(value).map_err(|_| error())?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|f| f.write_all(&bytes))
        .map_err(|_| error())
}
fn error() -> AppError {
    AppError::new(
        "AUTOMATION_STORAGE_ERROR",
        "无法保存自动追剧进度，已停止安排下一步",
    )
}
