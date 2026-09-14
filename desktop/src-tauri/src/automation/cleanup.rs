use super::*;

fn read_json(path: &Path) -> Option<Value> {
    if fs::metadata(path).ok()?.len() > 4 * 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn walk(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
    dirs: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    safe_root(dir)?;
    if !fs::canonicalize(dir)
        .map_err(|_| invalid_file())?
        .starts_with(root)
    {
        return Err(invalid_file());
    }
    for entry in fs::read_dir(dir).map_err(|_| invalid_file())? {
        let path = entry.map_err(|_| invalid_file())?.path();
        safe_root(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| invalid_file())?;
        if metadata.is_dir() {
            walk(root, &path, files, dirs)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            return Err(invalid_file());
        }
    }
    dirs.push(dir.to_owned());
    Ok(())
}

fn auxiliary(task: &Task, root: &Path, path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if path.parent() == Some(root) {
        if name == "自动追剧记录.json" {
            return false;
        }
        if name == "发布文案.json" {
            return read_json(path).is_some_and(|v| text(&v, "taskId") == task.id);
        }
        if matches!(name, "简介.txt" | "详细简介.txt") {
            return true;
        }
        let title = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if ["jpg", "jpeg", "png", "webp"]
            .iter()
            .any(|ext| name == format!("{title}.{ext}"))
        {
            return true;
        }
    }
    if (name.starts_with("下载完成-") && name.ends_with(".json")) || name == "首集副本凭据.json"
    {
        return fs::read(path)
            .ok()
            .and_then(|v| serde_json::from_slice::<OwnedFile>(&v).ok())
            .is_some_and(|record| {
                task.owned.iter().any(|owned| {
                    owned.path == record.path
                        && owned.hash == record.hash
                        && owned.size == record.size
                })
            });
    }
    if name == "result.json" {
        return read_json(path).is_some_and(|v| {
            v["identity"]["source"]["book_id"]
                .as_str()
                .is_some_and(|book| book.starts_with(&format!("auto-{}-", task.id)))
        });
    }
    false
}

pub(super) fn finish(task: &mut Task) -> Result<String, AppError> {
    safe_root(&task.root)?;
    if !task.root.exists() {
        return Ok("上传完成，任务文件夹已删除，去重记录保留在应用中".into());
    }
    if !flag(&task.config, "deleteEpisodes") || !flag(&task.config, "deleteFinal") {
        return Ok("上传完成，已按设置清理；任务文件夹保留（未同时启用删除单集和成片）".into());
    }
    let root = fs::canonicalize(&task.root).map_err(|_| invalid_file())?;
    let marker = root.join("自动追剧记录.json");
    let matches = read_json(&marker)
        .is_some_and(|v| text(&v, "id") == task.id && text(&v, "bookId") == task.book_id);
    let auto_root = root.parent().ok_or_else(invalid_file)?;
    if auto_root.file_name().is_none_or(|n| n != "自动追剧")
        || task.id.len() < 8
        || !root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&task.id[..8])
    {
        return Ok("上传完成，媒体已清理；文件夹缺少匹配的任务标记，已保留".into());
    }
    // Recover a crash after removing the final marker but before removing the
    // now-empty task directory. The expected root name and parent were checked.
    if !marker.exists()
        && fs::read_dir(&root)
            .map_err(|_| invalid_file())?
            .next()
            .is_none()
    {
        fs::remove_dir(&root).map_err(|_| invalid_file())?;
        return Ok("上传完成，任务文件夹已删除，去重记录保留在应用中".into());
    }
    if !matches {
        return Ok("上传完成，媒体已清理；文件夹缺少匹配的任务标记，已保留".into());
    }
    let mut files = vec![];
    let mut dirs = vec![];
    walk(&root, &root, &mut files, &mut dirs)?;
    let archive = auto_root.join("字幕留存").join(root.file_name().unwrap());
    let mut kept = 0;
    let mut unknown = 0;
    for path in files {
        if path == marker {
            continue;
        }
        let subtitle = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("srt" | "vtt")
        );
        let owned = task.owned.iter().find(|f| f.path == path);
        if let Some(original) = owned.filter(|_| subtitle && flag(&task.config, "keepSubtitles")) {
            let actual = identity(&path)?;
            if actual.hash != original.hash || actual.size != original.size {
                return Err(invalid_file());
            }
            let target = archive.join(path.strip_prefix(&root).map_err(|_| invalid_file())?);
            if !target.starts_with(&archive) || !archive.starts_with(auto_root) {
                return Err(invalid_file());
            }
            safe_root(&target)?;
            fs::create_dir_all(target.parent().ok_or_else(invalid_file)?)
                .map_err(|_| invalid_file())?;
            safe_root(&target)?;
            if !target.exists() {
                crate::atomic_write(
                    &target,
                    &fs::read(&path).map_err(|_| invalid_file())?,
                    "保留字幕",
                )?;
            }
            let copied = identity(&target)?;
            if copied.hash != original.hash || copied.size != original.size {
                return Err(invalid_file());
            }
            safe_root(&path)?;
            fs::remove_file(&path).map_err(|_| invalid_file())?;
            if task.subtitle.as_ref() == Some(&path) {
                task.subtitle = Some(target);
            }
            kept += 1;
        } else if auxiliary(task, &root, &path) {
            fs::remove_file(path).map_err(|_| invalid_file())?;
        } else {
            unknown += 1;
        }
    }
    // Only remove empty directories, bottom-up; untracked files keep the root.
    for dir in dirs.iter().filter(|dir| **dir != root) {
        safe_root(dir)?;
        if fs::read_dir(dir)
            .map_err(|_| invalid_file())?
            .next()
            .is_none()
        {
            fs::remove_dir(dir).map_err(|_| invalid_file())?;
        }
    }
    if unknown > 0 {
        return Ok(format!(
            "上传完成，媒体已清理；文件夹中有 {unknown} 个未登记文件，已保留。{}",
            if kept > 0 {
                "字幕已移至字幕留存"
            } else {
                ""
            }
        ));
    }
    if !read_json(&marker)
        .is_some_and(|v| text(&v, "id") == task.id && text(&v, "bookId") == task.book_id)
    {
        return Err(invalid_file());
    }
    safe_root(&root)?;
    fs::remove_file(marker).map_err(|_| invalid_file())?;
    fs::remove_dir(&root).map_err(|_| invalid_file())?;
    Ok(if kept > 0 {
        format!(
            "上传完成，任务文件夹已删除；{kept} 份字幕已移至 {}，去重记录保留在应用中",
            archive.display()
        )
    } else {
        "上传完成，任务文件夹已删除，去重记录保留在应用中".into()
    })
}
