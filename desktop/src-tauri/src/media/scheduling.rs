//! Admission control for independent separation jobs. Resource estimates are
//! conservative reservations, not guarantees about model peak allocations.
use super::{model::MediaJobKind, tools::background_command, MediaJob};
use std::{
    io::Read,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use sysinfo::System;

pub const MAX_AI_JOBS: usize = 5;
pub(crate) const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ExecutionBudget {
    pub cpu_threads: usize,
    pub force_cpu: bool,
    pub(crate) merge: bool,
    pub(crate) memory: u64,
    pub(crate) gpu_memory: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GpuResources {
    pub free_memory: u64,
    pub usage: f32,
    pub shared_memory: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Resources {
    pub cores: usize,
    pub cpu_usage: Option<f32>,
    pub available_memory: Option<u64>,
    pub gpu: Option<GpuResources>,
}

pub(crate) type ResourceProbe = Box<dyn FnMut() -> Resources + Send>;

pub(crate) fn native_probe() -> ResourceProbe {
    let mut system = System::new();
    system.refresh_cpu_usage();
    let mut refreshed = Instant::now();
    let mut sampled = false;
    let mut cached = None;
    Box::new(move || {
        if let Some(value) = cached {
            if !cfg!(windows) && refreshed.elapsed() < Duration::from_secs(2) {
                return value;
            }
        }
        // CPU usage requires two observations; never treat the warmup zero as idle.
        if refreshed.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            system.refresh_cpu_usage();
            sampled = refreshed.elapsed() < Duration::from_secs(10);
        }
        system.refresh_memory();
        let memory = system.available_memory();
        let value = Resources {
            cores: thread::available_parallelism().map_or(1, usize::from),
            cpu_usage: sampled.then(|| system.global_cpu_usage()),
            available_memory: (memory > 0).then_some(memory),
            gpu: if cfg!(target_os = "macos") {
                apple_gpu_resources(memory)
            } else {
                nvidia_resources()
            },
        };
        cached = Some(value);
        refreshed = Instant::now();
        value
    })
}

fn nvidia_resources() -> Option<GpuResources> {
    if cfg!(target_os = "macos") || std::env::var_os("CUDA_VISIBLE_DEVICES").is_some() {
        return None;
    }
    // Only admit GPU concurrency when the device mapping is unambiguous: a
    // single NVIDIA GPU, without CUDA visibility remapping. Multiple CSV rows
    // deliberately fail parsing and retain single-task execution.
    // A missing/stalled driver must never stall shutdown or queue controls.
    let output = probe_output(
        "nvidia-smi",
        &[
            "--query-gpu=memory.free,utilization.gpu",
            "--format=csv,noheader,nounits",
        ],
    )?;
    parse_gpu_resources(&output)
}

fn apple_gpu_resources(available_memory: u64) -> Option<GpuResources> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = probe_output("/usr/sbin/ioreg", &["-r", "-c", "IOAccelerator", "-d", "1"])?;
    let usage = parse_apple_gpu_usage(&output)?;
    Some(GpuResources {
        free_memory: available_memory,
        usage,
        shared_memory: true,
    })
}

fn parse_apple_gpu_usage(output: &str) -> Option<f32> {
    // Driver registry counters are optional. Treat absent/changed keys as
    // unknown, never as an idle GPU. On multiple entries use the busiest one.
    output
        .lines()
        .filter(|line| line.contains("\"PerformanceStatistics\""))
        .filter_map(|line| {
            let value = line.split_once("\"Device Utilization %\"=")?.1;
            let number: String = value
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            let usage = number.parse::<f32>().ok()?;
            (usage <= 100.0).then_some(usage)
        })
        .max_by(f32::total_cmp)
}

fn probe_output(program: &str, args: &[&str]) -> Option<String> {
    let mut child = background_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    // Drain concurrently: ioreg output may exceed a pipe's capacity.
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout.take(256 * 1024).read_to_string(&mut output).ok()?;
        Some(output)
    });
    let started = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < Duration::from_millis(800) => {
                thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let output = reader.join().ok().flatten();
    if success {
        output
    } else {
        None
    }
}

fn parse_gpu_resources(output: &str) -> Option<GpuResources> {
    let mut fields = output.trim().split(',').map(str::trim);
    let free_memory = fields
        .next()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024 * 1024)?;
    let usage = fields.next()?.parse::<f32>().ok()?;
    if fields.next().is_some() || !usage.is_finite() || !(0.0..=100.0).contains(&usage) {
        return None;
    }
    Some(GpuResources {
        free_memory,
        usage,
        shared_memory: false,
    })
}

pub(crate) fn is_parallel_kind(kind: MediaJobKind) -> bool {
    parallel_kind_for_platform(kind, cfg!(target_os = "windows"))
}

fn parallel_kind_for_platform(kind: MediaJobKind, windows: bool) -> bool {
    kind == MediaJobKind::SeparateBackgroundMusic
        || (windows && kind == MediaJobKind::ExtractSubtitles)
}

#[cfg(test)]
fn admit_legacy(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
) -> Option<ExecutionBudget> {
    admit_for_platform(job, active, resources, false)
}

#[cfg(any(windows, test))]
pub(crate) fn cpu_share(active: usize) -> u32 {
    // Job object rates are hundredths of the machine's CPU capacity.
    8500 / active.max(1) as u32
}

pub(crate) fn admit_configured(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
    concurrency: usize,
) -> Result<ExecutionBudget, &'static str> {
    configured_for_platform(job, active, resources, concurrency, cfg!(windows))
}

fn configured_for_platform(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
    concurrency: usize,
    windows: bool,
) -> Result<ExecutionBudget, &'static str> {
    if windows && job.kind == MediaJobKind::Merge {
        return admit_merge(job, active, resources);
    }
    if windows
        && active.iter().any(|b| b.merge)
        && (!resources
            .available_memory
            .is_some_and(|free| free >= memory_reservation(job, false) + 2 * GIB)
            || !resources.cpu_usage.is_some_and(|usage| {
                usage.is_finite()
                    && (0.0..85.0).contains(&usage)
                    && resources.cores as f32 * (1.0 - usage / 100.0) >= 3.0
            }))
    {
        return Err("合并运行中，等待足够的 CPU 和内存启动 AI；下载与上传继续运行");
    }
    if !parallel_kind_for_platform(job.kind, windows) && !active.is_empty() {
        return Err("合并任务等待其他媒体任务完成");
    }
    let limit = if concurrency > 0 {
        concurrency.min(if windows { 10 } else { MAX_AI_JOBS })
    } else {
        MAX_AI_JOBS
    };
    if active.iter().filter(|b| !windows || !b.merge).count() >= limit {
        return Err("已达到同时处理上限；完成一个任务后自动补位（暂停任务仍占用名额）");
    }
    // Only auto may change devices. Keep the saved preference intact and carry
    // the actual execution choice to the worker through its transient budget.
    let auto = windows
        && parallel_kind_for_platform(job.kind, true)
        && job.ai_request.as_ref().is_some_and(|r| r.device == "auto");
    let gpu_busy = resources.gpu.is_some_and(|gpu| {
        !gpu.shared_memory && (gpu.usage >= 85.0 || gpu.free_memory < gpu_reservation(job) + GIB)
    });
    if auto && (gpu_busy || (resources.gpu.is_none() && !active.is_empty())) {
        let mut cpu_job = job.clone();
        cpu_job.ai_request.as_mut().unwrap().device = "cpu".into();
        // Use full model RAM/load checks even under a manual concurrency limit.
        return admit_with_limit(&cpu_job, active, resources, true, limit)
            .filter(|budget| {
                resources
                    .available_memory
                    .is_some_and(|free| free >= budget.memory + 2 * GIB)
                    && resources.cpu_usage.is_some_and(|usage| {
                        usage.is_finite()
                            && (0.0..85.0).contains(&usage)
                            && resources.cores as f32 * (1.0 - usage / 100.0)
                                >= (budget.cpu_threads + 1) as f32
                    })
            })
            .map(|mut budget| {
                budget.force_cpu = true;
                budget
            })
            .ok_or("GPU 暂无余量，等待可用 CPU 或内存后继续处理");
    }
    if windows && concurrency > 0 && parallel_kind_for_platform(job.kind, true) {
        // A user-selected limit overrides conservative model estimates, while
        // preserving actual OS/VRAM headroom. Unknown telemetry is shown in UI.
        if resources.available_memory.is_some_and(|v| v < 2 * GIB) {
            return Err("可用内存不足 2 GB，等待运行中的任务释放内存");
        }
        let gpu_requested = job.ai_request.as_ref().is_some_and(|r| r.device != "cpu");
        if gpu_requested && resources.gpu.is_some_and(|g| g.free_memory < GIB) {
            return Err("可用显存不足 1 GB，等待运行中的任务释放显存");
        }
        let memory = if auto {
            memory_reservation(job, false)
        } else {
            2 * GIB
        };
        if auto
            && resources
                .available_memory
                .is_some_and(|free| free < memory + 2 * GIB)
        {
            return Err("可用内存不足，等待运行中的任务释放模型内存");
        }
        return Ok(ExecutionBudget {
            cpu_threads: (resources.cores.saturating_mul(85) / 100 / limit)
                .clamp(1, 8)
                .min(resources.cores.max(1)),
            force_cpu: false,
            merge: false,
            memory,
            gpu_memory: if auto {
                gpu_reservation(job)
            } else if gpu_requested {
                GIB
            } else {
                0
            },
        });
    }
    admit_for_platform(job, active, resources, windows).ok_or_else(|| {
        if job.ai_request.as_ref().is_some_and(|r| r.device != "cpu") && resources.gpu.is_none() {
            "自动模式未读取到显卡资源，暂按单任务处理；可手动指定同时处理数"
        } else {
            "自动模式等待可用 CPU、内存或显存；可调整同时处理数"
        }
    })
}

fn admit_merge(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
) -> Result<ExecutionBudget, &'static str> {
    if active.iter().any(|b| b.merge) {
        return Err("已有合并任务运行，等待合并通道空闲；其他资源可继续处理");
    }
    let copy_only = job.merge_request.as_ref().is_some_and(|r| {
        !r.square_canvas
            && (r.mode == Some(super::model::MergeMode::Copy)
                || r.mode.is_none() && !r.transcode_h264)
    });
    let gpu_ready = resources
        .gpu
        .is_some_and(|g| !g.shared_memory && g.usage < 85.0 && g.free_memory >= 2 * GIB);
    let force_cpu = copy_only || !gpu_ready && (!active.is_empty() || resources.gpu.is_some());
    let cpu_threads = if copy_only {
        1
    } else if gpu_ready {
        2
    } else {
        (resources.cores / 3).clamp(1, 4)
    }
    .min(resources.cores.max(1));
    let memory = if copy_only { GIB / 2 } else { GIB };
    if !active.is_empty()
        && (!resources
            .available_memory
            .is_some_and(|free| free >= memory + 2 * GIB)
            || !resources.cpu_usage.is_some_and(|usage| {
                usage.is_finite()
                    && (0.0..85.0).contains(&usage)
                    && resources.cores as f32 * (1.0 - usage / 100.0) >= (cpu_threads + 1) as f32
            }))
    {
        return Err("合并等待可用 CPU 或内存，下载和上传通道继续运行");
    }
    Ok(ExecutionBudget {
        cpu_threads,
        force_cpu,
        merge: true,
        memory,
        gpu_memory: if force_cpu { 0 } else { GIB },
    })
}

fn admit_for_platform(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
    windows: bool,
) -> Option<ExecutionBudget> {
    admit_with_limit(job, active, resources, windows, MAX_AI_JOBS)
}

fn admit_with_limit(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
    windows: bool,
    limit: usize,
) -> Option<ExecutionBudget> {
    if !parallel_kind_for_platform(job.kind, windows) {
        return active.is_empty().then_some(ExecutionBudget {
            cpu_threads: 1,
            force_cpu: false,
            merge: job.kind == MediaJobKind::Merge,
            memory: 0,
            gpu_memory: 0,
        });
    }
    if active.iter().filter(|b| !windows || !b.merge).count() >= limit {
        return None;
    }
    let Some(request) = job.ai_request.as_ref() else {
        // Let the executor fail legacy jobs with missing persisted requests.
        return active.is_empty().then_some(ExecutionBudget {
            cpu_threads: 1,
            force_cpu: false,
            merge: false,
            memory: 0,
            gpu_memory: 0,
        });
    };
    let gpu_requested = request.device != "cpu";
    let cuda = gpu_requested && resources.gpu.is_some_and(|gpu| !gpu.shared_memory);
    let mps = gpu_requested && resources.gpu.is_some_and(|gpu| gpu.shared_memory);
    // Every process also reserves CPU and RAM: an auto accelerator can fall back
    // to CPU, and paused workers retain their model allocations.
    let cpu_threads = if cuda || mps {
        2
    } else {
        (resources.cores / 3).clamp(1, 4)
    }
    .min(resources.cores.max(1));
    // Conservative per-process reservations include decoding and CPU fallback.
    let memory = memory_reservation(job, mps);
    let gpu_memory = if cuda { gpu_reservation(job) } else { 0 };
    let budget = ExecutionBudget {
        cpu_threads,
        force_cpu: false,
        merge: false,
        memory,
        gpu_memory,
    };
    if let Some(available) = resources.available_memory {
        let reserved: u64 = if windows {
            0
        } else {
            active.iter().map(|item| item.memory).sum()
        };
        // Preserve the existing single-job path on smaller machines. Additional
        // jobs need the full reservation plus OS headroom.
        if available < 2 * GIB
            || (!active.is_empty() && available.saturating_sub(reserved) < memory + 2 * GIB)
        {
            return None;
        }
    } else if !active.is_empty() {
        return None;
    }
    if let Some(gpu) = resources.gpu.filter(|_| gpu_requested) {
        let reserved: u64 = if windows {
            0
        } else {
            active.iter().map(|item| item.gpu_memory).sum()
        };
        if !active.is_empty()
            && (gpu.usage >= 85.0
                || (cuda && gpu.free_memory.saturating_sub(reserved) < gpu_memory + GIB))
        {
            return None;
        }
    } else if gpu_requested && !active.is_empty() {
        // Unknown CUDA telemetry: one worker only, including missing drivers.
        return None;
    }
    if !active.is_empty() {
        let usage = resources
            .cpu_usage
            .filter(|usage| usage.is_finite() && (0.0..=100.0).contains(usage))?;
        let total_threads: usize =
            active.iter().map(|item| item.cpu_threads).sum::<usize>() + cpu_threads;
        let ceiling = (resources.cores * 4 / 5).max(1);
        let idle = resources.cores as f32 * (1.0 - usage / 100.0);
        if total_threads > ceiling || idle < (cpu_threads + 1) as f32 {
            return None;
        }
    }
    Some(budget)
}

fn gpu_reservation(job: &MediaJob) -> u64 {
    let Some(request) = &job.ai_request else {
        return 0;
    };
    (if job.kind == MediaJobKind::ExtractSubtitles {
        if request.model == "medium" {
            6
        } else {
            3
        }
    } else if request.model == "htdemucs_ft" {
        6
    } else {
        4
    }) * GIB
}

fn memory_reservation(job: &MediaJob, mps: bool) -> u64 {
    let Some(request) = &job.ai_request else {
        return 0;
    };
    (if job.kind == MediaJobKind::ExtractSubtitles {
        if request.model == "medium" {
            8
        } else {
            4
        }
    } else if mps {
        if request.model == "htdemucs_ft" {
            14
        } else {
            10
        }
    } else if request.model == "htdemucs_ft" {
        10
    } else {
        6
    }) * GIB
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_manual_limit_keeps_running_slots_until_completion() {
        let j = job("cpu", "htdemucs");
        let first = configured_for_platform(&j, &[], idle(), 1, false).unwrap();
        assert!(configured_for_platform(&j, &[first], idle(), 1, false).is_err());
        assert!(configured_for_platform(&j, &[], idle(), 1, false).is_ok());
        assert!(configured_for_platform(&j, &[first; 5], idle(), 10, false).is_err());
    }

    #[test]
    fn windows_manual_slots_allocate_cpu_threads_from_machine_capacity() {
        let resources = Resources {
            cores: 20,
            ..idle()
        };
        let request = job("cpu", "htdemucs");
        let one = configured_for_platform(&request, &[], resources, 1, true).unwrap();
        let five = configured_for_platform(&request, &[], resources, 5, true).unwrap();
        assert!(one.cpu_threads > five.cpu_threads);
        assert!(five.cpu_threads > 1);
        assert!(five.cpu_threads * 5 <= 17);
        let small =
            configured_for_platform(&request, &[], Resources { cores: 2, ..idle() }, 10, true)
                .unwrap();
        assert_eq!(small.cpu_threads, 1);
    }

    #[test]
    fn windows_auto_places_work_on_cpu_while_gpu_is_full() {
        let request = job("auto", "htdemucs");
        let resources = Resources {
            cores: 20,
            gpu: Some(GpuResources {
                free_memory: 8 * GIB,
                usage: 0.0,
                shared_memory: false,
            }),
            ..idle()
        };
        let gpu = configured_for_platform(&request, &[], resources, 0, true).unwrap();
        assert!(!gpu.force_cpu);
        let manual_gpu = configured_for_platform(&request, &[], resources, 5, true).unwrap();
        let after_launch = Resources {
            gpu: Some(GpuResources {
                free_memory: 8 * GIB - manual_gpu.gpu_memory,
                ..resources.gpu.unwrap()
            }),
            ..resources
        };
        assert!(
            configured_for_platform(&request, &[manual_gpu], after_launch, 5, true)
                .unwrap()
                .force_cpu
        );
        let busy = Resources {
            gpu: Some(GpuResources {
                free_memory: 2 * GIB,
                usage: 95.0,
                shared_memory: false,
            }),
            ..resources
        };
        for slots in [0, 5] {
            let cpu = configured_for_platform(&request, &[gpu], busy, slots, true).unwrap();
            assert!(cpu.force_cpu);
            assert_eq!(cpu.gpu_memory, 0);
            assert!(cpu.cpu_threads > 1);
            assert_eq!(request.ai_request.as_ref().unwrap().device, "auto");
            assert!(configured_for_platform(
                &request,
                &[gpu],
                Resources {
                    cpu_usage: Some(98.0),
                    ..busy
                },
                slots,
                true
            )
            .is_err());
            assert!(configured_for_platform(
                &request,
                &[gpu],
                Resources {
                    available_memory: Some(3 * GIB),
                    ..busy
                },
                slots,
                true
            )
            .is_err());
        }
        assert!(configured_for_platform(&job("cuda", "htdemucs"), &[gpu], busy, 0, true).is_err());
        assert!(configured_for_platform(&request, &[gpu], busy, 0, false).is_err());
    }
    fn job(device: &str, model: &str) -> MediaJob {
        serde_json::from_value(serde_json::json!({
            "id":"test", "dedupeKey":"test", "kind":"separateBackgroundMusic", "status":"queued",
            "stage":"queued", "percent":0, "inputs":[],
            "aiRequest": {"bookId":"test", "title":"test", "kind":"separateBackgroundMusic",
                "scope":"merged", "seriesRoot":"test", "inputs":[], "model":model, "device":device, "dedupeKey":"test"}
        })).unwrap()
    }

    fn idle() -> Resources {
        Resources {
            cores: 64,
            cpu_usage: Some(0.0),
            available_memory: Some(128 * GIB),
            gpu: None,
        }
    }

    #[test]
    fn windows_merge_uses_spare_gpu_while_cpu_ai_is_running_even_with_one_ai_slot() {
        let resources = Resources {
            cores: 16,
            gpu: Some(GpuResources {
                free_memory: 8 * GIB,
                usage: 0.0,
                shared_memory: false,
            }),
            ..idle()
        };
        let ai = configured_for_platform(&job("cpu", "htdemucs"), &[], resources, 1, true).unwrap();
        let mut merge = job("cpu", "htdemucs");
        merge.kind = MediaJobKind::Merge;
        merge.ai_request = None;
        let budget = configured_for_platform(&merge, &[ai], resources, 1, true)
            .expect("idle GPU must not wait for CPU separation");
        assert!(budget.gpu_memory > 0);
        assert!(!budget.force_cpu);
        assert!(configured_for_platform(
            &merge,
            &[ai],
            Resources {
                cpu_usage: Some(98.0),
                ..resources
            },
            1,
            true
        )
        .is_err());
        assert!(configured_for_platform(
            &merge,
            &[ai],
            Resources {
                available_memory: Some(GIB),
                ..resources
            },
            1,
            true
        )
        .is_err());
        assert!(configured_for_platform(&merge, &[budget], resources, 1, true).is_err());
        assert!(
            configured_for_platform(&job("cpu", "htdemucs"), &[budget], resources, 1, true).is_ok()
        );
        let busy_gpu = Resources {
            gpu: Some(GpuResources {
                free_memory: GIB,
                usage: 99.0,
                shared_memory: false,
            }),
            ..resources
        };
        let cpu_merge = configured_for_platform(&merge, &[ai], busy_gpu, 1, true).unwrap();
        assert!(cpu_merge.force_cpu);
        assert_eq!(cpu_merge.gpu_memory, 0);
        let cpu_ai =
            configured_for_platform(&job("auto", "htdemucs"), &[budget], busy_gpu, 1, true)
                .unwrap();
        assert!(cpu_ai.force_cpu);
    }

    #[test]
    fn windows_explicit_slots_override_estimates_and_keep_real_headroom() {
        let job = job("cuda", "htdemucs");
        let resources = Resources {
            cores: 8,
            cpu_usage: Some(15.0),
            available_memory: Some(8 * GIB),
            gpu: Some(GpuResources {
                free_memory: 2 * GIB,
                usage: 15.0,
                shared_memory: false,
            }),
        };
        let first = configured_for_platform(&job, &[], resources, 5, true).unwrap();
        assert!(configured_for_platform(&job, &[first; 4], resources, 5, true).is_ok());
        assert!(
            configured_for_platform(&job, &[first; 5], resources, 5, true)
                .unwrap_err()
                .contains("上限")
        );
        // Lowering the limit stops new admission, without cancelling existing jobs.
        assert!(configured_for_platform(&job, &[first; 4], resources, 2, true).is_err());
        assert!(configured_for_platform(&job, &[first], resources, 2, true).is_ok());
        assert!(configured_for_platform(&job, &[first; 9], resources, 10, true).is_ok());
        let scarce = Resources {
            available_memory: Some(GIB),
            ..resources
        };
        assert!(configured_for_platform(&job, &[first], scarce, 5, true)
            .unwrap_err()
            .contains("内存"));
        let scarce = Resources {
            gpu: Some(GpuResources {
                free_memory: GIB / 2,
                ..resources.gpu.unwrap()
            }),
            ..resources
        };
        assert!(configured_for_platform(&job, &[first], scarce, 5, true)
            .unwrap_err()
            .contains("显存"));
        // The Windows-only override must never alter macOS admission.
        assert!(configured_for_platform(&job, &[first], resources, 5, false).is_err());
    }

    #[test]
    fn windows_does_not_deduct_running_allocations_from_live_free_memory_twice() {
        let job = job("cuda", "htdemucs");
        let resources = Resources {
            available_memory: Some(10 * GIB),
            gpu: Some(GpuResources {
                free_memory: 6 * GIB,
                usage: 15.0,
                shared_memory: false,
            }),
            ..idle()
        };
        let first = admit_for_platform(&job, &[], resources, true).unwrap();
        assert!(admit_for_platform(&job, &[first], resources, true).is_some());
        assert!(admit_for_platform(&job, &[first], resources, false).is_none());
    }

    #[test]
    fn cpu_share_releases_capacity_to_the_final_task() {
        assert_eq!(cpu_share(5), 1700);
        assert_eq!(cpu_share(2), 4250);
        assert_eq!(cpu_share(1), 8500);
        assert_eq!(cpu_share(0), 8500);
    }

    #[test]
    fn cpu_admission_obeys_live_load_ram_core_budget_and_hard_cap() {
        let job = job("cpu", "htdemucs");
        let resources = idle();
        let first = admit_legacy(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 4);
        assert!(admit_legacy(&job, &[first; 4], resources).is_some());
        assert!(admit_legacy(&job, &[first; 5], resources).is_none());
        for scarce in [
            Resources {
                cpu_usage: Some(98.0),
                ..resources
            },
            Resources {
                available_memory: Some(8 * GIB),
                ..resources
            },
            Resources {
                cores: 4,
                ..resources
            },
            Resources {
                cpu_usage: None,
                ..resources
            },
            Resources {
                available_memory: None,
                ..resources
            },
        ] {
            assert!(admit_legacy(&job, &[first], scarce).is_none());
        }
        // A single task can still try on a smaller machine, as before.
        assert!(admit_legacy(
            &job,
            &[],
            Resources {
                cores: 2,
                available_memory: Some(3 * GIB),
                ..resources
            }
        )
        .is_some());
    }

    #[test]
    fn cuda_admission_obeys_live_vram_and_load_with_unknown_probe_fallback() {
        let job = job("cuda", "htdemucs");
        let gpu = GpuResources {
            free_memory: 24 * GIB,
            usage: 0.0,
            shared_memory: false,
        };
        let resources = Resources {
            gpu: Some(gpu),
            ..idle()
        };
        let first = admit_legacy(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 2);
        assert!(admit_legacy(&job, &[first; 4], resources).is_some());
        for scarce in [
            Resources {
                gpu: Some(GpuResources {
                    free_memory: 6 * GIB,
                    ..gpu
                }),
                ..resources
            },
            Resources {
                gpu: Some(GpuResources { usage: 99.0, ..gpu }),
                ..resources
            },
            Resources {
                gpu: None,
                ..resources
            },
        ] {
            assert!(admit_legacy(&job, &[first], scarce).is_none());
            assert!(
                admit_legacy(&job, &[], scarce).is_some(),
                "single task retains accelerator/CPU fallback"
            );
        }
    }

    #[test]
    fn fine_tuned_model_requires_more_ram_and_vram() {
        let standard = job("cuda", "htdemucs");
        let fine = job("cuda", "htdemucs_ft");
        let resources = Resources {
            gpu: Some(GpuResources {
                free_memory: 10 * GIB,
                usage: 0.0,
                shared_memory: false,
            }),
            ..idle()
        };
        let first = admit_legacy(&standard, &[], resources).unwrap();
        assert!(admit_legacy(&standard, &[first], resources).is_some());
        assert!(admit_legacy(&fine, &[first], resources).is_none());
    }

    #[test]
    fn apple_gpu_uses_live_utilization_and_unified_memory() {
        let job = job("mps", "htdemucs");
        let gpu = GpuResources {
            free_memory: 128 * GIB,
            usage: 12.0,
            shared_memory: true,
        };
        let resources = Resources {
            gpu: Some(gpu),
            ..idle()
        };
        let first = admit_legacy(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 2);
        assert!(admit_legacy(&job, &[first; 4], resources).is_some());
        assert!(admit_legacy(
            &job,
            &[first],
            Resources {
                available_memory: Some(16 * GIB),
                ..resources
            }
        )
        .is_none());
        assert!(admit_legacy(
            &job,
            &[first],
            Resources {
                gpu: Some(GpuResources { usage: 95.0, ..gpu }),
                ..resources
            }
        )
        .is_none());
        assert_eq!(
            parse_apple_gpu_usage(
                r#""PerformanceStatistics" = {"Device Utilization %"=45,"In use system memory"=417169408}"#
            ),
            Some(45.0)
        );
        assert_eq!(
            parse_apple_gpu_usage(r#""PerformanceStatistics" = {"Renderer Utilization %"=45}"#),
            None
        );
    }

    #[test]
    fn merge_and_subtitles_never_overlap_with_separation() {
        let separate = job("cpu", "htdemucs");
        let first = admit_legacy(&separate, &[], idle()).unwrap();
        for kind in [MediaJobKind::Merge, MediaJobKind::ExtractSubtitles] {
            let mut exclusive = separate.clone();
            exclusive.kind = kind;
            assert!(admit_for_platform(&exclusive, &[first], idle(), false).is_none());
            assert!(admit_legacy(&exclusive, &[], idle()).is_some());
        }
    }

    #[test]
    fn windows_subtitles_share_ai_capacity_while_macos_retains_exclusive_subtitles() {
        let separation = job("cpu", "htdemucs");
        let first = admit_for_platform(&separation, &[], idle(), true).unwrap();
        let mut subtitle = job("cpu", "small");
        subtitle.kind = MediaJobKind::ExtractSubtitles;
        subtitle.ai_request.as_mut().unwrap().kind = MediaJobKind::ExtractSubtitles;
        assert!(parallel_kind_for_platform(subtitle.kind, true));
        assert!(!parallel_kind_for_platform(subtitle.kind, false));
        assert!(!parallel_kind_for_platform(MediaJobKind::Merge, true));
        let second = admit_for_platform(&subtitle, &[first], idle(), true).unwrap();
        assert_eq!(second.memory, 4 * GIB);
        assert!(admit_for_platform(&subtitle, &[second; 4], idle(), true).is_some());
        assert!(admit_for_platform(&subtitle, &[second; 5], idle(), true).is_none());
        assert!(admit_for_platform(&subtitle, &[first], idle(), false).is_none());
        let mac = admit_for_platform(&subtitle, &[], idle(), false).unwrap();
        assert_eq!(mac.cpu_threads, 1);
        assert_eq!(mac.memory, 0);
        for scarce in [
            Resources {
                available_memory: Some(5 * GIB),
                ..idle()
            },
            Resources {
                cpu_usage: Some(98.0),
                ..idle()
            },
            Resources {
                cpu_usage: None,
                ..idle()
            },
        ] {
            assert!(admit_for_platform(&subtitle, &[first], scarce, true).is_none());
        }
        subtitle.ai_request.as_mut().unwrap().device = "cuda".into();
        assert!(admit_for_platform(&subtitle, &[first], idle(), true).is_none());
        let gpu = Resources {
            gpu: Some(GpuResources {
                free_memory: 10 * GIB,
                usage: 0.0,
                shared_memory: false,
            }),
            ..idle()
        };
        let first = admit_for_platform(&separation, &[], gpu, true).unwrap();
        assert!(admit_for_platform(&subtitle, &[first], gpu, true).is_some());
        subtitle.ai_request.as_mut().unwrap().model = "medium".into();
        let medium = admit_for_platform(&subtitle, &[], gpu, true).unwrap();
        assert_eq!(medium.memory, 8 * GIB);
        assert_eq!(medium.gpu_memory, 6 * GIB);
        assert!(admit_for_platform(
            &subtitle,
            &[medium],
            Resources {
                gpu: Some(GpuResources {
                    free_memory: 6 * GIB,
                    ..gpu.gpu.unwrap()
                }),
                ..gpu
            },
            true
        )
        .is_none());
    }

    #[test]
    fn gpu_telemetry_rejects_unavailable_and_invalid_readings() {
        let gpu = parse_gpu_resources(" 16384, 12 \n").unwrap();
        assert_eq!(gpu.free_memory, 16 * GIB);
        assert_eq!(gpu.usage, 12.0);
        for invalid in [
            "N/A, 0",
            "1024, N/A",
            "1024, NaN",
            "1024, 101",
            "-1, 0",
            "1, 0, 2",
        ] {
            assert!(parse_gpu_resources(invalid).is_none());
        }
    }
}
