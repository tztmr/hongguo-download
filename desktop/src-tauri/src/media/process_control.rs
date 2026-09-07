use crate::AppError;
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Debug, Default)]
struct ProcessControlState {
    cancelled: AtomicBool,
    paused: AtomicBool,
    process_group: Mutex<Option<i32>>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessControl(Arc<ProcessControlState>);

pub type CancellationToken = ProcessControl;

impl ProcessControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prepare_command(&self, command: &mut Command) {
        command.process_group(0);
    }

    pub fn register_child(&self, child: &mut Child) -> Result<(), AppError> {
        let pgid = i32::try_from(child.id()).map_err(|_| signal_error("invalid child id"))?;
        *self
            .0
            .process_group
            .lock()
            .map_err(|_| unavailable_error())? = Some(pgid);
        if self.0.paused.load(Ordering::Acquire) {
            signal_group(pgid, libc::SIGSTOP)?;
        }
        if self.0.cancelled.load(Ordering::Acquire) {
            let _ = signal_group(pgid, libc::SIGCONT);
            let _ = signal_group(pgid, libc::SIGTERM);
        }
        Ok(())
    }

    pub fn clear_child(&self, child_id: u32) {
        if let Ok(mut guard) = self.0.process_group.lock() {
            if *guard == i32::try_from(child_id).ok() {
                *guard = None;
            }
        }
    }

    pub fn pause(&self) -> Result<(), AppError> {
        self.0.paused.store(true, Ordering::Release);
        if let Some(pgid) = self.current_process_group()? {
            if let Err(error) = signal_group(pgid, libc::SIGSTOP) {
                self.0.paused.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn resume(&self) -> Result<(), AppError> {
        if let Some(pgid) = self.current_process_group()? {
            signal_group(pgid, libc::SIGCONT)?;
        }
        self.0.paused.store(false, Ordering::Release);
        Ok(())
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        if let Ok(Some(pgid)) = self.current_process_group() {
            let _ = signal_group(pgid, libc::SIGCONT);
            let _ = signal_group(pgid, libc::SIGTERM);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Acquire)
    }

    fn current_process_group(&self) -> Result<Option<i32>, AppError> {
        self.0
            .process_group
            .lock()
            .map(|guard| *guard)
            .map_err(|_| unavailable_error())
    }
}

fn signal_group(pgid: i32, signal: i32) -> Result<(), AppError> {
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(signal_error(io::Error::last_os_error()))
    }
}

fn unavailable_error() -> AppError {
    AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
}

fn signal_error(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_JOB_SIGNAL_FAILED",
        "无法控制媒体处理进程",
        cause.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::ProcessControl;
    use std::{
        fs,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    fn temp_heartbeat(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hongguo-process-control-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn wait_for_size(path: &std::path::Path, greater_than: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let size = fs::metadata(path).map(|value| value.len()).unwrap_or(0);
            if size > greater_than {
                return size;
            }
            assert!(Instant::now() < deadline, "heartbeat did not grow");
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn spawn_heartbeat(control: &ProcessControl, path: &std::path::Path) -> std::process::Child {
        let script = format!(
            "while true; do printf x >> '{}'; sleep 0.05; done",
            path.display()
        );
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        control.prepare_command(&mut command);
        let mut child = command.spawn().unwrap();
        control.register_child(&mut child).unwrap();
        child
    }

    #[test]
    fn pause_stops_process_group_and_resume_continues_same_child() {
        let heartbeat = temp_heartbeat("pause-resume");
        let control = ProcessControl::new();
        let mut child = spawn_heartbeat(&control, &heartbeat);
        let before_pause = wait_for_size(&heartbeat, 2);

        control.pause().unwrap();
        thread::sleep(Duration::from_millis(180));
        let stopped_size = fs::metadata(&heartbeat).unwrap().len();
        thread::sleep(Duration::from_millis(180));
        assert_eq!(fs::metadata(&heartbeat).unwrap().len(), stopped_size);
        assert!(stopped_size >= before_pause);

        control.resume().unwrap();
        wait_for_size(&heartbeat, stopped_size);
        control.cancel();
        let _ = child.wait();
        control.clear_child(child.id());
        let _ = fs::remove_file(heartbeat);
    }

    #[test]
    fn child_registered_after_pause_is_stopped_immediately() {
        let heartbeat = temp_heartbeat("pause-before-spawn");
        let control = ProcessControl::new();
        control.pause().unwrap();
        let mut child = spawn_heartbeat(&control, &heartbeat);
        thread::sleep(Duration::from_millis(180));
        assert_eq!(
            fs::metadata(&heartbeat)
                .map(|value| value.len())
                .unwrap_or(0),
            0
        );

        control.resume().unwrap();
        wait_for_size(&heartbeat, 0);
        control.cancel();
        let _ = child.wait();
        control.clear_child(child.id());
        let _ = fs::remove_file(heartbeat);
    }
}
