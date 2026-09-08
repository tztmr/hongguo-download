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

pub const MAX_SEPARATION_JOBS: usize = 5;
pub(crate) const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ExecutionBudget {
    pub cpu_threads: usize,
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
            if refreshed.elapsed() < Duration::from_secs(2) {
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

pub(crate) fn admit(
    job: &MediaJob,
    active: &[ExecutionBudget],
    resources: Resources,
) -> Option<ExecutionBudget> {
    if job.kind != MediaJobKind::SeparateBackgroundMusic {
        return active.is_empty().then_some(ExecutionBudget {
            cpu_threads: 1,
            memory: 0,
            gpu_memory: 0,
        });
    }
    if active.len() >= MAX_SEPARATION_JOBS {
        return None;
    }
    let Some(request) = job.ai_request.as_ref() else {
        // Let the executor fail legacy jobs with missing persisted requests.
        return active.is_empty().then_some(ExecutionBudget {
            cpu_threads: 1,
            memory: 0,
            gpu_memory: 0,
        });
    };
    let fine_tuned = request.model == "htdemucs_ft";
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
    let memory = (if mps {
        if fine_tuned {
            14
        } else {
            10
        }
    } else if fine_tuned {
        10
    } else {
        6
    }) * GIB;
    let gpu_memory = if cuda {
        (if fine_tuned { 6 } else { 4 }) * GIB
    } else {
        0
    };
    let budget = ExecutionBudget {
        cpu_threads,
        memory,
        gpu_memory,
    };
    if let Some(available) = resources.available_memory {
        let reserved: u64 = active.iter().map(|item| item.memory).sum();
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
        let reserved: u64 = active.iter().map(|item| item.gpu_memory).sum();
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn cpu_admission_obeys_live_load_ram_core_budget_and_hard_cap() {
        let job = job("cpu", "htdemucs");
        let resources = idle();
        let first = admit(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 4);
        assert!(admit(&job, &[first; 4], resources).is_some());
        assert!(admit(&job, &[first; 5], resources).is_none());
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
            assert!(admit(&job, &[first], scarce).is_none());
        }
        // A single task can still try on a smaller machine, as before.
        assert!(admit(
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
        let first = admit(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 2);
        assert!(admit(&job, &[first; 4], resources).is_some());
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
            assert!(admit(&job, &[first], scarce).is_none());
            assert!(
                admit(&job, &[], scarce).is_some(),
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
        let first = admit(&standard, &[], resources).unwrap();
        assert!(admit(&standard, &[first], resources).is_some());
        assert!(admit(&fine, &[first], resources).is_none());
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
        let first = admit(&job, &[], resources).unwrap();
        assert_eq!(first.cpu_threads, 2);
        assert!(admit(&job, &[first; 4], resources).is_some());
        assert!(admit(
            &job,
            &[first],
            Resources {
                available_memory: Some(16 * GIB),
                ..resources
            }
        )
        .is_none());
        assert!(admit(
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
        let first = admit(&separate, &[], idle()).unwrap();
        for kind in [MediaJobKind::Merge, MediaJobKind::ExtractSubtitles] {
            let mut exclusive = separate.clone();
            exclusive.kind = kind;
            assert!(admit(&exclusive, &[first], idle()).is_none());
            assert!(admit(&exclusive, &[], idle()).is_some());
        }
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
