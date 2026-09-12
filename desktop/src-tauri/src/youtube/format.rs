use crate::{media::MediaTools, AppError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, process::Stdio};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UploadFormat {
    #[default]
    Auto,
    Shorts,
    Standard,
}

impl UploadFormat {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

pub fn validate_file(format: UploadFormat, path: &Path) -> Result<(), AppError> {
    if format == UploadFormat::Auto {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|_| invalid())?;
    let tools = MediaTools::from_resource_root(exe.parent().ok_or_else(invalid)?)?;
    validate_with_tools(format, path, &tools)
}

pub fn validate_with_tools(
    format: UploadFormat,
    path: &Path,
    tools: &MediaTools,
) -> Result<(), AppError> {
    if format == UploadFormat::Auto {
        return Ok(());
    }
    let output = tools
        .ffprobe_command()
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| invalid())?;
    if !output.status.success() {
        return Err(invalid());
    }
    let data: Value = serde_json::from_slice(&output.stdout).map_err(|_| invalid())?;
    validate_probe(format, &data)
}
fn invalid() -> AppError {
    AppError::new(
        "UPLOAD_FORMAT_PROBE_FAILED",
        "无法确认视频画幅或时长，请检查视频与媒体工具",
    )
}

fn validate_probe(format: UploadFormat, data: &Value) -> Result<(), AppError> {
    let stream = data["streams"]
        .as_array()
        .and_then(|v| {
            v.iter().find(|s| {
                s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_u64() != Some(1)
            })
        })
        .ok_or_else(invalid)?;
    let mut width = stream["width"].as_f64().ok_or_else(invalid)?;
    let mut height = stream["height"].as_f64().ok_or_else(invalid)?;
    if let Some(sar) = stream["sample_aspect_ratio"]
        .as_str()
        .filter(|s| *s != "N/A" && *s != "0:1")
    {
        let (n, d) = sar.split_once(':').ok_or_else(invalid)?;
        let n: f64 = n.parse().map_err(|_| invalid())?;
        let d: f64 = d.parse().map_err(|_| invalid())?;
        if n <= 0.0 || d <= 0.0 {
            return Err(invalid());
        }
        width *= n / d;
    }
    let rotation = stream["side_data_list"]
        .as_array()
        .and_then(|v| v.iter().find_map(|s| s["rotation"].as_f64()))
        .or_else(|| {
            stream["tags"]["rotate"]
                .as_str()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0.0);
    if (rotation.abs() % 180.0 - 90.0).abs() < 1.0 {
        std::mem::swap(&mut width, &mut height);
    }
    let duration: f64 = data["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(invalid)?;
    if !duration.is_finite()
        || duration <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return Err(invalid());
    }
    let is_short = width <= height && duration <= 180.0;
    match (format, is_short) {
        (UploadFormat::Shorts, false) => Err(AppError::new(
            "UPLOAD_SHORTS_FORMAT_REQUIRED",
            "Shorts 需要竖版或方形、时长不超过 3 分钟；请先准备符合条件的视频",
        )),
        (UploadFormat::Standard, true) => Err(AppError::new(
            "UPLOAD_STANDARD_FORMAT_REQUIRED",
            "此视频会被识别为 Shorts；普通视频请使用横版或超过 3 分钟的成片",
        )),
        _ => Ok(()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn checks_duration_and_both_upload_modes() {
        for (w, h, d, short) in [
            (1080, 1920, 180.0, true),
            (1080, 1080, 30.0, true),
            (1080, 1920, 180.01, false),
            (1920, 1080, 30.0, false),
        ] {
            let v = json!({"streams":[{"codec_type":"video","width":w,"height":h}],"format":{"duration":d.to_string()}});
            assert_eq!(validate_probe(UploadFormat::Shorts, &v).is_ok(), short);
            assert_eq!(validate_probe(UploadFormat::Standard, &v).is_ok(), !short);
        }
    }
    #[test]
    fn respects_rotation_and_non_square_pixels() {
        let mut v = json!({"streams":[{"codec_type":"video","width":1920,"height":1080,"side_data_list":[{"rotation":-90}]}],"format":{"duration":"59"}});
        assert!(validate_probe(UploadFormat::Shorts, &v).is_ok());
        v["streams"][0] =
            json!({"codec_type":"video","width":720,"height":720,"sample_aspect_ratio":"2:1"});
        assert!(validate_probe(UploadFormat::Shorts, &v).is_err());
    }
    #[test]
    fn rejects_unknown_or_invalid_duration() {
        for d in ["0", "NaN", "N/A"] {
            let v = json!({"streams":[{"codec_type":"video","width":1080,"height":1920}],"format":{"duration":d}});
            assert!(validate_probe(UploadFormat::Shorts, &v).is_err());
        }
    }
}
