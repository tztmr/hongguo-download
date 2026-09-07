use crate::AppError;
use std::{
    mem::size_of,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            OpenThread, ResumeThread, SuspendThread, CREATE_NEW_PROCESS_GROUP,
            THREAD_SUSPEND_RESUME,
        },
    },
};

#[derive(Debug)]
struct JobHandle(HANDLE);

impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[derive(Debug, Default)]
struct ProcessControlState {
    cancelled: AtomicBool,
    paused: AtomicBool,
    process: Mutex<Option<(u32, JobHandle)>>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessControl(Arc<ProcessControlState>);

pub type CancellationToken = ProcessControl;

impl ProcessControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prepare_command(&self, command: &mut Command) {
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    pub fn register_child(&self, child: &mut Child) -> Result<(), AppError> {
        let job = create_kill_on_close_job()?;
        let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            return Err(control_error(std::io::Error::last_os_error()));
        }
        let pid = child.id();
        *self.0.process.lock().map_err(|_| unavailable_error())? = Some((pid, job));
        if self.0.paused.load(Ordering::Acquire) {
            suspend_process_threads(pid)?;
        }
        if self.0.cancelled.load(Ordering::Acquire) {
            self.terminate_current();
        }
        Ok(())
    }

    pub fn clear_child(&self, child_id: u32) {
        if let Ok(mut guard) = self.0.process.lock() {
            if guard.as_ref().map(|(pid, _)| *pid) == Some(child_id) {
                *guard = None;
            }
        }
    }

    pub fn pause(&self) -> Result<(), AppError> {
        self.0.paused.store(true, Ordering::Release);
        if let Some(pid) = self.current_pid()? {
            if let Err(error) = suspend_process_threads(pid) {
                self.0.paused.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn resume(&self) -> Result<(), AppError> {
        if let Some(pid) = self.current_pid()? {
            resume_process_threads(pid)?;
        }
        self.0.paused.store(false, Ordering::Release);
        Ok(())
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.terminate_current();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Acquire)
    }

    fn current_pid(&self) -> Result<Option<u32>, AppError> {
        self.0
            .process
            .lock()
            .map(|guard| guard.as_ref().map(|(pid, _)| *pid))
            .map_err(|_| unavailable_error())
    }

    fn terminate_current(&self) {
        if let Ok(guard) = self.0.process.lock() {
            if let Some((_, job)) = guard.as_ref() {
                unsafe { TerminateJobObject(job.0, 1) };
            }
        }
    }
}

fn create_kill_on_close_job() -> Result<JobHandle, AppError> {
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    let job = JobHandle(handle);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    Ok(job)
}

fn suspend_process_threads(pid: u32) -> Result<(), AppError> {
    visit_process_threads(pid, |thread| unsafe {
        if SuspendThread(thread) == u32::MAX {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

fn resume_process_threads(pid: u32) -> Result<(), AppError> {
    visit_process_threads(pid, |thread| unsafe {
        if ResumeThread(thread) == u32::MAX {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

fn visit_process_threads(
    pid: u32,
    mut action: impl FnMut(HANDLE) -> Result<(), std::io::Error>,
) -> Result<(), AppError> {
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(control_error)?;
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    let snapshot_guard = JobHandle(snapshot);
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut has_entry = unsafe { Thread32First(snapshot_guard.0, &mut entry) }.is_ok();
    while has_entry {
        if entry.th32OwnerProcessID == pid {
            found = true;
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !thread.is_null() {
                let thread_guard = JobHandle(thread);
                action(thread_guard.0).map_err(control_error)?;
            }
        }
        has_entry = unsafe { Thread32Next(snapshot_guard.0, &mut entry) }.is_ok();
    }
    if !found {
        return Err(control_error("media process has no controllable threads"));
    }
    Ok(())
}

fn unavailable_error() -> AppError {
    AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
}

fn control_error(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_JOB_SIGNAL_FAILED",
        "无法控制媒体处理进程",
        cause.to_string(),
    )
}
