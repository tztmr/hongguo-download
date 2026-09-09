use crate::AppError;
use std::{
    mem::size_of,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
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
            AssignProcessToJobObject, CreateJobObjectW, JobObjectCpuRateControlInformation,
            JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            OpenThread, ResumeThread, SuspendThread, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
            THREAD_SUSPEND_RESUME,
        },
    },
};

#[derive(Debug)]
struct JobHandle(usize);

impl JobHandle {
    fn new(handle: HANDLE) -> Self {
        Self(handle as usize)
    }

    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.raw()) };
    }
}

#[derive(Debug, Default)]
struct ProcessControlState {
    cancelled: AtomicBool,
    paused: AtomicBool,
    cpu_rate: AtomicU32,
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
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    pub fn register_child(&self, child: &mut Child) -> Result<(), AppError> {
        let job = create_kill_on_close_job()?;
        let assigned =
            unsafe { AssignProcessToJobObject(job.raw(), child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            return Err(control_error(std::io::Error::last_os_error()));
        }
        let pid = child.id();
        {
            let mut guard = self.0.process.lock().map_err(|_| unavailable_error())?;
            apply_cpu_rate(&job, self.0.cpu_rate.load(Ordering::Acquire))?;
            *guard = Some((pid, job));
        }
        if self.0.paused.load(Ordering::Acquire) {
            suspend_process_threads(pid)?;
        }
        if self.0.cancelled.load(Ordering::Acquire) {
            self.terminate_current();
        }
        Ok(())
    }

    pub fn set_cpu_rate(&self, rate: u32) -> Result<(), AppError> {
        let guard = self.0.process.lock().map_err(|_| unavailable_error())?;
        let rate = rate.clamp(1, 10000);
        if self.0.cpu_rate.load(Ordering::Acquire) == rate {
            return Ok(());
        }
        if let Some((_, job)) = guard.as_ref() {
            apply_cpu_rate(job, rate)?;
        }
        self.0.cpu_rate.store(rate, Ordering::Release);
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

    pub fn kill(&self) {
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
                unsafe { TerminateJobObject(job.raw(), 1) };
            }
        }
    }
}

fn apply_cpu_rate(job: &JobHandle, rate: u32) -> Result<(), AppError> {
    if rate == 0 {
        return Ok(());
    }
    let mut limits = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
        ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        ..Default::default()
    };
    limits.Anonymous.CpuRate = rate;
    let result = unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectCpuRateControlInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
        )
    };
    if result == 0 {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn create_kill_on_close_job() -> Result<JobHandle, AppError> {
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    let job = JobHandle::new(handle);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job.raw(),
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
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(control_error(std::io::Error::last_os_error()));
    }
    let snapshot_guard = JobHandle::new(snapshot);
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut has_entry = unsafe { Thread32First(snapshot_guard.raw(), &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == pid {
            found = true;
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !thread.is_null() {
                let thread_guard = JobHandle::new(thread);
                action(thread_guard.raw()).map_err(control_error)?;
            }
        }
        has_entry = unsafe { Thread32Next(snapshot_guard.raw(), &mut entry) } != 0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::JobObjects::QueryInformationJobObject;

    fn configured_rate(control: &ProcessControl) -> u32 {
        let guard = control.0.process.lock().unwrap();
        let (_, job) = guard.as_ref().unwrap();
        let mut info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        let result = unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectCpuRateControlInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(result, 0);
        assert_eq!(
            info.ControlFlags,
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
        );
        unsafe { info.Anonymous.CpuRate }
    }

    #[test]
    fn live_worker_gains_freed_cpu_and_later_child_keeps_the_rate() {
        let control = ProcessControl::new();
        control
            .set_cpu_rate(super::super::scheduling::cpu_share(5))
            .unwrap();
        for _ in 0..2 {
            let mut command = Command::new("cmd.exe");
            command.args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"]);
            control.prepare_command(&mut command);
            let mut child = command.spawn().unwrap();
            control.register_child(&mut child).unwrap();
            assert!(matches!(configured_rate(&control), 1700 | 8500));
            control
                .set_cpu_rate(super::super::scheduling::cpu_share(1))
                .unwrap();
            assert_eq!(configured_rate(&control), 8500);
            assert!(
                child.try_wait().unwrap().is_none(),
                "rebalance must not restart the worker"
            );
            control.kill();
            child.wait().unwrap();
            control.clear_child(child.id());
        }
    }
}
