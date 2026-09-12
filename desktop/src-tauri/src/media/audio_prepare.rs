//! Cancellable audio decoding with live progress and bounded FFmpeg diagnostics.
use super::{CancellationToken, MediaTools};
use crate::AppError;
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

pub(super) fn extract(
    tools: &MediaTools,
    input: &Path,
    output: &Path,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(String, f64),
) -> Result<(), AppError> {
    if cancellation.is_cancelled() {
        return Err(cancelled());
    }
    progress("正在检测音轨".into(), 0.0);
    let probe = tools.probe_media(input)?;
    if probe.audio.is_none() {
        return Err(AppError::new(
            "AI_AUDIO_STREAM_MISSING",
            "源视频没有音轨，无法分离背景音乐或识别语音；请检查对应剧集",
        ));
    }
    progress("正在预处理音频 0%".into(), 0.0);
    let mut command = tools.ffmpeg_command();
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-fflags",
            "+genpts",
            "-i",
        ])
        .arg(dunce::simplified(input))
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-af",
            "aresample=async=1:first_pts=0",
            "-c:a",
            "pcm_s16le",
            "-rf64",
            "auto",
            "-f",
            "wav",
            "-progress",
            "pipe:1",
            "-nostats",
        ])
        .arg(dunce::simplified(output))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cancellation.prepare_command(&mut command);
    let mut child = command.spawn().map_err(|e| {
        AppError::with_cause(
            "AI_AUDIO_PREPARE_FAILED",
            "无法启动音频预处理工具，请检查安装目录中的 FFmpeg",
            e.to_string(),
        )
    })?;
    let id = child.id();
    if let Err(error) = cancellation.register_child(&mut child) {
        let _ = child.kill();
        let _ = child.wait();
        cancellation.clear_child(id);
        return Err(error);
    }
    let elapsed = Arc::new(AtomicU64::new(0));
    let latest = elapsed.clone();
    let stdout = child.stdout.take().expect("piped progress");
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(us) = line
                .strip_prefix("out_time_us=")
                .and_then(|v| v.parse::<u64>().ok())
            {
                latest.store(us, Ordering::Relaxed);
            }
        }
    });
    let mut stderr = child.stderr.take().expect("piped diagnostics");
    let errors = thread::spawn(move || {
        let mut tail = Vec::new();
        let mut bytes = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut bytes) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&bytes[..n]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });
    let mut last_percent = 0;
    let outcome = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Err(cancelled());
        }
        let duration = probe.duration_seconds;
        let percent = if duration.is_finite() && duration > 0.0 {
            (elapsed.load(Ordering::Relaxed) as f64 / 1_000_000.0 / duration * 100.0)
                .clamp(0.0, 99.0) as u32
        } else {
            0
        };
        if percent > last_percent {
            last_percent = percent;
            progress(format!("正在预处理音频 {percent}%"), percent as f64);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(AppError::with_cause(
                    "AI_AUDIO_PREPARE_FAILED",
                    "无法读取音频预处理状态",
                    error.to_string(),
                ));
            }
        }
    };
    cancellation.clear_child(id);
    let _ = reader.join();
    let diagnostic = errors.join().unwrap_or_default();
    let status = outcome?;
    if !status.success() {
        return Err(failure(&diagnostic, &status.to_string()));
    }
    if !output.metadata().is_ok_and(|m| m.is_file() && m.len() > 44) {
        return Err(failure("empty audio output", &status.to_string()));
    }
    progress("音频预处理完成".into(), 100.0);
    Ok(())
}

fn cancelled() -> AppError {
    AppError::new("AI_CANCELLED", "AI 媒体任务已取消")
}

fn failure(diagnostic: &str, status: &str) -> AppError {
    let lower = diagnostic.to_lowercase();
    let reason = if lower.contains("no space left") || lower.contains("disk full") {
        "磁盘空间不足，请释放下载盘空间后重试"
    } else if lower.contains("permission denied") || lower.contains("access is denied") {
        "无法读写文件，请检查下载目录权限或文件占用"
    } else if lower.contains("no such file") || lower.contains("file not found") {
        "输入或临时文件不存在，请检查下载路径"
    } else if lower.contains("invalid data")
        || lower.contains("error while decoding")
        || lower.contains("moov atom")
    {
        "音频数据损坏或无法解码，请检查源视频并重新下载损坏剧集"
    } else if lower.contains("empty audio output") {
        "没有生成有效音频，请检查源视频的音轨"
    } else {
        "FFmpeg 无法解码音轨，源文件已保留；请检查音轨和下载目录"
    };
    let detail: String = diagnostic
        .lines()
        .rev()
        .find(|line| {
            !line.trim().is_empty()
                && !line.contains("Conversion failed")
                && !line.contains("Error opening output files")
        })
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(240)
        .collect();
    AppError::with_cause(
        "AI_AUDIO_PREPARE_FAILED",
        format!(
            "音频预处理失败：{reason}（{status}）{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!("；FFmpeg：{detail}")
            }
        ),
        diagnostic,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_failures_explain_disk_permissions_and_decode_errors() {
        for (diagnostic, message) in [
            ("No space left on device", "磁盘空间不足"),
            ("Permission denied", "目录权限"),
            ("Error while decoding stream", "音频数据损坏"),
        ] {
            let error = failure(diagnostic, "exit code: 1");
            assert!(error.message.contains(message));
            assert!(error.message.contains(diagnostic));
        }
        assert!(failure(&"x".repeat(20000), "1").message.chars().count() < 400);
    }

    #[test]
    fn audio_preparation_decodes_real_media_in_chinese_directory_and_emits_progress() {
        let Ok(root) = std::env::var("HONGGUO_TEST_PLAYBACK_TOOLS") else {
            return;
        };
        let tools = MediaTools::from_resource_root(root).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "hongguo-audio-中文-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("源视频.mp4");
        assert!(tools
            .ffmpeg_command()
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-c:a",
                "aac"
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success());
        let output = dir.join("预处理.wav");
        let mut events = Vec::new();
        extract(
            &tools,
            &std::fs::canonicalize(&input).unwrap(),
            &output,
            &CancellationToken::default(),
            &mut |s, p| events.push((s, p)),
        )
        .unwrap();
        assert!(events.first().unwrap().0.contains("检测音轨"));
        assert_eq!(events.last().unwrap().1, 100.0);
        assert!(events.windows(2).all(|w| w[0].1 <= w[1].1));
        let probe = tools
            .ffprobe_command()
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(&output)
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"][0]["sample_rate"], "16000");
        assert_eq!(value["streams"][0]["channels"], 1);
        assert!(
            (value["streams"][0]["duration"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
                - 2.0)
                .abs()
                < 0.1
        );
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        let unused = dir.join("不应生成.wav");
        assert_eq!(
            extract(&tools, &input, &unused, &cancelled, &mut |_, _| {})
                .unwrap_err()
                .code,
            "AI_CANCELLED"
        );
        assert!(!unused.exists());
        assert!(input.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
