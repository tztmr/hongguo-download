use super::tools::MediaTools;
use crate::AppError;
use serde::Serialize;
use std::{
    fs,
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    Copy,
    Nvenc,
    VideoToolbox,
    Libx264,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VideoHardwareCapabilities {
    pub nvenc_available: bool,
    pub encoder: String,
    pub failure_reason: Option<String>,
}

pub fn select_video_encoder(needs_transcode: bool, nvenc_available: bool) -> VideoEncoder {
    if !needs_transcode {
        VideoEncoder::Copy
    } else if nvenc_available {
        VideoEncoder::Nvenc
    } else if cfg!(target_os = "macos") {
        VideoEncoder::VideoToolbox
    } else {
        VideoEncoder::Libx264
    }
}

pub fn probe_video_hardware(tools: &MediaTools) -> VideoHardwareCapabilities {
    match probe_nvenc(tools) {
        Ok(()) => VideoHardwareCapabilities {
            nvenc_available: true,
            encoder: "h264_nvenc".into(),
            failure_reason: None,
        },
        Err(error) => VideoHardwareCapabilities {
            nvenc_available: false,
            encoder: if cfg!(target_os = "macos") {
                "h264_videotoolbox".into()
            } else {
                "libx264".into()
            },
            failure_reason: Some(format!("{}: {}", error.code, error.message)),
        },
    }
}

fn probe_nvenc(tools: &MediaTools) -> Result<(), AppError> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "hongguo-nvenc-probe-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).map_err(probe_failed)?;
    let output = directory.join("probe.mp4");
    let result = (|| {
        let status = tools
            .ffmpeg_command()
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x180:r=30",
                "-t",
                "0.25",
                "-an",
                "-c:v",
                "h264_nvenc",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(probe_failed)?;
        if !status.success() {
            return Err(probe_failed("FFmpeg NVENC probe failed"));
        }
        let probe = tools
            .ffprobe_command()
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height",
                "-of",
                "csv=p=0",
            ])
            .arg(&output)
            .output()
            .map_err(probe_failed)?;
        let value = String::from_utf8_lossy(&probe.stdout);
        if !probe.status.success() || !value.starts_with("h264,320,180") {
            return Err(probe_failed("NVENC output validation failed"));
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(directory);
    result
}

fn probe_failed(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_NVENC_UNAVAILABLE",
        "NVIDIA 视频编码不可用",
        cause.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{select_video_encoder, VideoEncoder};

    #[test]
    fn copy_path_never_requires_an_encoder_probe() {
        assert_eq!(select_video_encoder(false, true), VideoEncoder::Copy);
        assert_eq!(select_video_encoder(false, false), VideoEncoder::Copy);
    }

    #[test]
    fn available_nvenc_is_selected_for_transcoding() {
        assert_eq!(select_video_encoder(true, true), VideoEncoder::Nvenc);
    }

    #[test]
    fn unavailable_nvenc_uses_the_platform_software_or_native_fallback() {
        let selected = select_video_encoder(true, false);
        if cfg!(target_os = "macos") {
            assert_eq!(selected, VideoEncoder::VideoToolbox);
        } else {
            assert_eq!(selected, VideoEncoder::Libx264);
        }
    }
}
