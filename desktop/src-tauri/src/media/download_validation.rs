//! Validate the MP4 envelope by seeking over payloads, including 64-bit boxes.
//! ffprobe can read a faststart moov even when the following mdat is truncated.
use crate::AppError;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(crate) fn validate_mp4(path: &Path) -> Result<(), AppError> {
    let mut file = File::open(path).map_err(io_error)?;
    let length = file.metadata().map_err(io_error)?.len();
    let mut offset = 0u64;
    let mut movie = false;
    let mut media = false;
    while offset < length {
        if length - offset < 8 {
            return Err(incomplete(length, "MP4 尾部不完整"));
        }
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).map_err(io_error)?;
        let short_size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let mut header_size = 8;
        let size = match short_size {
            0 => length - offset,
            1 => {
                if length - offset < 16 {
                    return Err(incomplete(length, "MP4 大文件头不完整"));
                }
                let mut extended = [0u8; 8];
                file.read_exact(&mut extended).map_err(io_error)?;
                header_size = 16;
                u64::from_be_bytes(extended)
            }
            size => size as u64,
        };
        if size < header_size || size > length - offset {
            return Err(incomplete(length, "MP4 数据长度与文件大小不一致"));
        }
        movie |= &header[4..] == b"moov" && size > header_size;
        media |= &header[4..] == b"mdat" && size > header_size;
        offset += size;
    }
    if !movie || !media {
        return Err(incomplete(length, "缺少 MP4 视频索引或媒体数据"));
    }
    Ok(())
}

fn incomplete(size: u64, reason: &str) -> AppError {
    AppError::new(
        "DOWNLOAD_INCOMPLETE",
        format!("{reason}（{size} 字节），需重新下载该集"),
    )
}
fn io_error(error: std::io::Error) -> AppError {
    AppError::with_cause(
        "DOWNLOAD_FILE_UNREADABLE",
        "无法读取下载文件，请检查路径或权限",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::io::Write;

    #[test]
    fn rejects_faststart_file_with_truncated_media_payload() {
        let path =
            std::env::temp_dir().join(format!("hongguo-truncated-{}.mp4", std::process::id()));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&12u32.to_be_bytes());
        bytes.extend_from_slice(b"moovtest");
        bytes.extend_from_slice(&100u32.to_be_bytes());
        bytes.extend_from_slice(b"mdatpartial");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(validate_mp4(&path).unwrap_err().code, "DOWNLOAD_INCOMPLETE");
        bytes[12..16].copy_from_slice(&15u32.to_be_bytes());
        fs::write(&path, &bytes).unwrap();
        validate_mp4(&path).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn seeks_over_sparse_media_larger_than_four_gib_without_reading_payload() {
        let path = std::env::temp_dir().join(format!("hongguo-large-{}.mp4", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(&12u32.to_be_bytes()).unwrap();
        file.write_all(b"moovtest").unwrap();
        file.write_all(&1u32.to_be_bytes()).unwrap();
        file.write_all(b"mdat").unwrap();
        let size = (1u64 << 32) + 16;
        file.write_all(&size.to_be_bytes()).unwrap();
        file.set_len(size + 12).unwrap();
        drop(file);
        validate_mp4(&path).unwrap();
        fs::remove_file(path).unwrap();
    }
}
