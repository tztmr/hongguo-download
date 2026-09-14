use crate::AppError;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSignature {
    pub codec_name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub time_base: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaProbe {
    pub video: StreamSignature,
    pub audio: Option<StreamSignature>,
    pub duration_seconds: f64,
}

fn invalid(reason: &str) -> AppError {
    AppError::new("FFPROBE_INVALID", format!("媒体检测失败：{reason}"))
}

pub(super) fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|v| v.is_finite())
}

pub(super) fn ratio(value: &str) -> Option<f64> {
    let (a, b) = value.split_once('/')?;
    let a = a.parse::<f64>().ok()?;
    let b = b.parse::<f64>().ok()?;
    let value = a / b;
    (a > 0.0 && b > 0.0 && value.is_finite()).then_some(value)
}

fn positive(value: &Value) -> Option<f64> {
    number(value).filter(|v| *v > 0.0)
}

pub(super) fn stream_duration(stream: &Value) -> Option<f64> {
    positive(&stream["duration"]).or_else(|| {
        let duration = positive(&stream["duration_ts"])? * ratio(stream["time_base"].as_str()?)?;
        (duration.is_finite() && duration > 0.0).then_some(duration)
    })
}

pub(super) fn is_video(stream: &Value) -> bool {
    stream["codec_type"] == "video"
        && !["attached_pic", "timed_thumbnails", "still_image"]
            .iter()
            .any(|key| number(&stream["disposition"][key]) == Some(1.0))
}

pub(super) fn decode(bytes: &[u8]) -> Result<Value, AppError> {
    serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).map_err(|e| {
        AppError::with_cause(
            "FFPROBE_INVALID",
            "媒体检测失败：检测工具未返回有效 JSON",
            e.to_string(),
        )
    })
}

#[cfg(unix)]
pub(super) fn parse_probe_json(bytes: &[u8]) -> Result<MediaProbe, AppError> {
    parse_value(&decode(bytes)?)
}

pub(super) fn parse_value(raw: &Value) -> Result<MediaProbe, AppError> {
    let streams = raw["streams"]
        .as_array()
        .ok_or_else(|| invalid("没有媒体流信息"))?;
    let mut videos = streams.iter().filter(|stream| is_video(stream));
    let video = videos
        .next()
        .ok_or_else(|| invalid("没有可用视频流，文件可能未下载完整"))?;
    if videos.next().is_some() {
        return Err(invalid("存在多个主视频流，无法确定合并画面"));
    }
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let duration_seconds = positive(&raw["format"]["duration"])
        .or_else(|| stream_duration(video))
        .or_else(|| audio.and_then(stream_duration))
        .ok_or_else(|| invalid("缺少有效时长，文件可能未下载完整"))?;
    let codec = |stream: &Value| {
        stream["codec_name"]
            .as_str()
            .filter(|v| !v.is_empty() && *v != "unknown")
            .map(str::to_owned)
            .ok_or_else(|| {
                let kind = if stream["codec_type"] == "video" {
                    "视频"
                } else {
                    "音频"
                };
                let tag = stream["codec_tag_string"].as_str().unwrap_or("");
                if matches!(tag.to_ascii_lowercase().as_str(), "bvc2" | "bytevc2") {
                    AppError::new(
                        "MEDIA_CODEC_UNSUPPORTED",
                        "视频采用 ByteVC2（bvc2）编码，当前媒体工具不支持，需重新选择兼容视频源下载",
                    )
                } else {
                    let tag: String = tag.chars().filter(|c| !c.is_control()).take(24).collect();
                    let detail = if tag.is_empty() {
                        String::new()
                    } else {
                        format!("（编码标记：{tag}）")
                    };
                    invalid(&format!("无法识别{kind}编码{detail}"))
                }
            })
    };
    let dimension = |key: &str| {
        video[key]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| invalid("视频宽高无效，文件可能未下载完整"))
    };
    // Some MP4s expose a codec timebase (e.g. 90000/1) as r_frame_rate.
    // Use the measured average when that value cannot be a usable frame rate.
    let frame_rate = ["r_frame_rate", "avg_frame_rate"]
        .iter()
        .filter_map(|key| video[key].as_str())
        .find(|value| ratio(value).is_some_and(|fps| (1.0..=120.0).contains(&fps)))
        .or_else(|| {
            video["r_frame_rate"]
                .as_str()
                .filter(|v| ratio(v).is_some())
        })
        .map(str::to_owned);
    Ok(MediaProbe {
        video: StreamSignature {
            codec_name: codec(video)?,
            width: Some(dimension("width")?),
            height: Some(dimension("height")?),
            frame_rate,
            time_base: video["time_base"].as_str().map(str::to_owned),
            sample_rate: None,
            channels: None,
        },
        audio: audio
            .map(|stream| {
                Ok(StreamSignature {
                    codec_name: codec(stream)?,
                    width: None,
                    height: None,
                    frame_rate: None,
                    time_base: stream["time_base"].as_str().map(str::to_owned),
                    sample_rate: positive(&stream["sample_rate"])
                        .filter(|n| n.fract() == 0.0 && *n <= u32::MAX as f64)
                        .map(|n| n as u32),
                    channels: stream["channels"]
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok()),
                })
            })
            .transpose()?,
        duration_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bytevc2_is_an_unsupported_codec_not_a_truncated_or_zero_size_video() {
        let raw = json!({"streams":[
            {"codec_type":"video","codec_tag_string":"bvc2","width":1256,"height":720,"duration":"109.600000"},
            {"codec_type":"audio","codec_name":"aac","codec_tag_string":"mp4a","sample_rate":"44100","channels":2}
        ],"format":{"duration":"109.600000","size":"5972218"}});
        let error = parse_value(&raw).unwrap_err();
        assert_eq!(error.code, "MEDIA_CODEC_UNSUPPORTED");
        assert!(error.message.contains("ByteVC2"));
        let mut repaired = raw.clone();
        repaired["streams"][0]["codec_name"] = json!("hevc");
        repaired["streams"][0]["codec_tag_string"] = json!("hvc1");
        assert_eq!(parse_value(&repaired).unwrap().video.codec_name, "hevc");
        repaired["streams"][0]["codec_name"] = json!("unknown");
        let unknown = parse_value(&repaired).unwrap_err();
        assert_eq!(unknown.code, "FFPROBE_INVALID");
        assert!(unknown.message.contains("hvc1"));
    }

    #[test]
    fn handles_optional_metadata_and_real_average_rate_without_guessing_duration() {
        for rate in ["0/0", "90000/1", "N/A"] {
            let data = json!({"streams":[
                {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,
                 "r_frame_rate":rate,"avg_frame_rate":"30000/1001",
                 "duration":"N/A","duration_ts":180000,"time_base":"1/90000"},
                {"codec_type":"audio","codec_name":"aac","sample_rate":"N/A","channels":2}
            ], "format":{"duration":"N/A"}});
            let parsed = parse_value(&data).unwrap();
            assert_eq!(parsed.video.frame_rate.as_deref(), Some("30000/1001"));
            assert_eq!(parsed.duration_seconds, 2.0);
            assert_eq!(parsed.audio.unwrap().sample_rate, None);
        }
    }

    #[test]
    fn supports_numeric_duration_but_rejects_missing_or_invalid_video() {
        let mut data = json!({"streams":[{"codec_type":"video","codec_name":"hevc",
            "width":1080,"height":1920,"duration":3.5}], "format":{}});
        assert_eq!(parse_value(&data).unwrap().duration_seconds, 3.5);
        data["streams"][0]["duration"] = json!("N/A");
        assert!(parse_value(&data).unwrap_err().message.contains("时长"));
        data["format"]["duration"] = json!(3.5);
        data["streams"][0]["width"] = json!(0);
        assert!(parse_value(&data).unwrap_err().message.contains("宽高"));
    }
}
