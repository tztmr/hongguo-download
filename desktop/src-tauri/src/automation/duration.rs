use super::*;

pub(super) const MAX_SECONDS: f64 = 12.0 * 60.0 * 60.0;

fn valid(seconds: f64) -> Result<f64, AppError> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(AppError::new(
            "AUTOMATION_DURATION_UNKNOWN",
            "无法确定视频时长，保留文件等待重试",
        ));
    }
    Ok(seconds)
}

pub(super) fn inspect(
    path: &Path,
    cached: Option<DurationCheck>,
    mut probe: impl FnMut(&Path) -> Result<f64, AppError>,
) -> Result<Option<DurationCheck>, AppError> {
    safe_root(path)?;
    let metadata = match fs::metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid_file()),
    };
    if !metadata.is_file() {
        return Err(invalid_file());
    }
    let modified_unix_nanos = metadata
        .modified()
        .map_err(|_| invalid_file())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid_file())?
        .as_nanos();
    if let Some(cached) = cached {
        if cached.path == path
            && cached.size == metadata.len()
            && cached.modified_unix_nanos == modified_unix_nanos
            && valid(cached.seconds).is_ok()
        {
            return Ok(Some(cached));
        }
    }
    Ok(Some(DurationCheck {
        path: path.into(),
        size: metadata.len(),
        modified_unix_nanos,
        seconds: valid(probe(path)?)?,
    }))
}

pub(super) fn inputs_over_limit(
    paths: &[PathBuf],
    mut probe: impl FnMut(&Path) -> Result<f64, AppError>,
) -> Result<Option<f64>, AppError> {
    let mut total = 0.0;
    for path in paths {
        safe_root(path)?;
        total += valid(probe(path)?)?;
        if total > MAX_SECONDS {
            return Ok(Some(total));
        }
    }
    Ok(None)
}

pub(super) fn skip(task: &mut Task, seconds: f64) -> bool {
    if task.main_done || !seconds.is_finite() || seconds <= MAX_SECONDS {
        return false;
    }
    task.status = Status::Skipped;
    task.message = format!(
        "已检测视频总时长 {:.2} 小时，超过 12 小时上限，自动跳过；保留已下载文件",
        seconds / 3600.0
    );
    task.media_state = None;
    task.attempts = 0;
    task.retry_at = 0;
    task.retry_ready = false;
    true
}
