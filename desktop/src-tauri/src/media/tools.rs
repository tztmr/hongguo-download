use super::merge::{probe_media, MediaProbe};
use crate::AppError;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone)]
pub struct MediaTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

pub(crate) fn background_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command
}

impl MediaTools {
    #[cfg(test)]
    pub(crate) fn from_test_paths(ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        Self { ffmpeg, ffprobe }
    }

    pub fn from_resource_root(root: impl AsRef<Path>) -> Result<Self, AppError> {
        let root = root.as_ref();
        let canonical_root = fs::canonicalize(root).map_err(|error| {
            AppError::with_cause(
                "MEDIA_TOOL_MISSING",
                "媒体工具资源目录不可用",
                error.to_string(),
            )
        })?;
        let ffmpeg = root.join(tool_name("ffmpeg"));
        let ffprobe = root.join(tool_name("ffprobe"));
        for path in [&ffmpeg, &ffprobe] {
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                AppError::with_cause("MEDIA_TOOL_MISSING", "缺少打包媒体工具", error.to_string())
            })?;
            if unsafe_tool_metadata(&metadata) {
                return Err(AppError::with_cause(
                    "MEDIA_TOOL_UNSAFE",
                    "打包媒体工具不得是链接",
                    path.display().to_string(),
                ));
            }
            if !metadata.is_file() {
                return Err(AppError::with_cause(
                    "MEDIA_TOOL_MISSING",
                    "打包媒体工具不是文件",
                    path.display().to_string(),
                ));
            }
            let canonical_path = fs::canonicalize(path).map_err(|error| {
                AppError::with_cause(
                    "MEDIA_TOOL_UNSAFE",
                    "无法验证打包媒体工具",
                    error.to_string(),
                )
            })?;
            if !canonical_path.starts_with(&canonical_root) {
                return Err(AppError::with_cause(
                    "MEDIA_TOOL_UNSAFE",
                    "打包媒体工具越出资源目录",
                    path.display().to_string(),
                ));
            }
            if !tool_is_executable(&metadata) {
                return Err(AppError::with_cause(
                    "MEDIA_TOOL_NOT_EXECUTABLE",
                    "打包媒体工具不可执行",
                    path.display().to_string(),
                ));
            }
        }
        Ok(Self { ffmpeg, ffprobe })
    }

    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }

    pub(crate) fn ffmpeg_command(&self) -> Command {
        background_command(&self.ffmpeg)
    }

    pub(crate) fn ffprobe_command(&self) -> Command {
        background_command(&self.ffprobe)
    }

    pub fn probe_media(&self, path: impl AsRef<Path>) -> Result<MediaProbe, AppError> {
        probe_media(self, path.as_ref())
    }
}

#[cfg(windows)]
fn tool_name(stem: &str) -> String {
    format!("{stem}.exe")
}

#[cfg(not(windows))]
fn tool_name(stem: &str) -> String {
    stem.to_owned()
}

#[cfg(unix)]
fn unsafe_tool_metadata(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || metadata.nlink() != 1
}

#[cfg(windows)]
fn unsafe_tool_metadata(metadata: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(unix)]
fn tool_is_executable(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn tool_is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(all(test, windows))]
mod windows_console_tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn console_probe() {
        if std::env::var_os("HONGGUO_CONSOLE_PROBE").is_none() {
            return;
        }
        assert!(unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null());
        println!("hidden-console-pipe-ok");
    }

    #[test]
    fn background_tools_and_controlled_workers_have_no_console_and_keep_pipes() {
        let executable = std::env::current_exe().unwrap();
        let tools = MediaTools::from_test_paths(executable.clone(), executable.clone());
        let mut controlled_worker = Command::new(&executable);
        crate::media::ProcessControl::new().prepare_command(&mut controlled_worker);
        let mut controlled_ffmpeg = tools.ffmpeg_command();
        crate::media::ProcessControl::new().prepare_command(&mut controlled_ffmpeg);
        for mut command in [
            background_command(&executable),
            controlled_worker,
            controlled_ffmpeg,
            tools.ffprobe_command(),
        ] {
            let output = command
                .args([
                    "--exact",
                    "media::tools::windows_console_tests::console_probe",
                    "--nocapture",
                ])
                .env("HONGGUO_CONSOLE_PROBE", "1")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
            assert!(String::from_utf8_lossy(&output.stdout).contains("hidden-console-pipe-ok"));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::MediaTools;
    use std::{
        fs,
        os::unix::fs::symlink,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct ToolFixture {
        root: PathBuf,
        outside: PathBuf,
    }

    impl ToolFixture {
        fn new() -> Self {
            let unique = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hongguo-media-tools-test-{}-{}-{unique}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("fixture clock should be after the Unix epoch")
                    .as_nanos()
            ));
            let outside = root.with_extension("outside");
            fs::create_dir_all(&root).expect("fixture directory should be created");
            fs::create_dir_all(&outside).expect("outside fixture directory should be created");
            Self { root, outside }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write_executable(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, b"fixture executable").expect("fixture tool should be written");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("fixture tool should be executable");
            path
        }

        fn write_outside_executable(&self, name: &str) -> PathBuf {
            let path = self.outside.join(name);
            fs::write(&path, b"outside fixture executable")
                .expect("outside fixture tool should be written");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("outside fixture tool should be executable");
            path
        }
    }

    impl Drop for ToolFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_dir_all(&self.outside);
        }
    }

    #[test]
    fn missing_tool_pair_is_rejected_with_stable_code() {
        // Production mutation caught: accepting a resource root with either bundled tool missing.
        let fixture = ToolFixture::new();
        fixture.write_executable("ffmpeg");

        let error = MediaTools::from_resource_root(fixture.root()).unwrap_err();

        assert_eq!(error.code, "MEDIA_TOOL_MISSING");
    }

    #[test]
    fn resource_root_errors_serialize_without_actual_paths() {
        // Production mutation caught: placing the actual packaged/user resource path in public AppError.message.
        let fixture = ToolFixture::new();
        let private_root = fixture.root().join("actual-user-private-tool-root");
        let marker = private_root.to_string_lossy().into_owned();

        let error = MediaTools::from_resource_root(&private_root).unwrap_err();
        let serialized = serde_json::to_string(&error).unwrap();

        assert_eq!(error.code, "MEDIA_TOOL_MISSING");
        assert!(!error.message.contains(&marker));
        assert!(!serialized.contains(&marker));
        assert!(!serialized.contains("actual-user-private-tool-root"));
    }

    #[test]
    fn non_executable_tool_is_rejected_with_stable_code() {
        // Production mutation caught: dropping the executable-bit validation for either tool.
        let fixture = ToolFixture::new();
        fixture.write_executable("ffmpeg");
        let ffprobe = fixture.root().join("ffprobe");
        fs::write(&ffprobe, b"fixture non-executable").expect("fixture tool should be written");
        fs::set_permissions(&ffprobe, fs::Permissions::from_mode(0o644))
            .expect("fixture permissions should be set");

        let error = MediaTools::from_resource_root(fixture.root()).unwrap_err();

        assert_eq!(error.code, "MEDIA_TOOL_NOT_EXECUTABLE");
    }

    #[test]
    fn valid_pair_resolves_exact_resource_paths() {
        // Production mutation caught: resolving either tool from PATH or a directory outside root.
        let fixture = ToolFixture::new();
        let ffmpeg = fixture.write_executable("ffmpeg");
        let ffprobe = fixture.write_executable("ffprobe");

        let tools = MediaTools::from_resource_root(fixture.root()).unwrap();

        assert_eq!(tools.ffmpeg(), ffmpeg.as_path());
        assert_eq!(tools.ffprobe(), ffprobe.as_path());
    }

    #[test]
    fn symlink_escape_is_rejected_without_following_the_target() {
        // Production mutation caught: replacing symlink_metadata with metadata and following outside root.
        let fixture = ToolFixture::new();
        let outside_ffmpeg = fixture.write_outside_executable("ffmpeg");
        symlink(outside_ffmpeg, fixture.root().join("ffmpeg"))
            .expect("fixture symlink should be created");
        fixture.write_executable("ffprobe");

        let error = MediaTools::from_resource_root(fixture.root()).unwrap_err();

        assert_eq!(error.code, "MEDIA_TOOL_UNSAFE");
    }

    #[test]
    fn hardlink_escape_is_rejected_even_when_the_target_is_regular() {
        // Production mutation caught: accepting a regular file whose inode is linked outside root.
        let fixture = ToolFixture::new();
        let outside_ffmpeg = fixture.write_outside_executable("ffmpeg");
        fs::hard_link(outside_ffmpeg, fixture.root().join("ffmpeg"))
            .expect("fixture hardlink should be created");
        fixture.write_executable("ffprobe");

        let error = MediaTools::from_resource_root(fixture.root()).unwrap_err();

        assert_eq!(error.code, "MEDIA_TOOL_UNSAFE");
    }
}
