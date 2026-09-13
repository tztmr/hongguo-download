use super::model::{number, text, Mode, Snapshot, Status, Task};
use std::collections::HashSet;

const GROUP_SIZE: usize = 10;

// Separate lanes keep slow network calls and media result polling from
// monopolising the task runner. The media service owns CPU/GPU admission.
fn lane(job: &Task) -> usize {
    match job.stage.as_str() {
        "download" => 0,
        "merge" => 1,
        "separate" => 2,
        "subtitles" => 3,
        "metadata" => 4,
        "upload" => 5,
        "short" if job.short_path.is_none() => 1,
        "short" => 5,
        "cleanup" | "done" => 6,
        _ => 7,
    }
}

pub(super) fn select(snapshot: &Snapshot, busy: &HashSet<String>, timestamp: u64) -> Vec<String> {
    if snapshot.mode != Mode::Running {
        return Vec::new();
    }
    let Some(config) = &snapshot.config else {
        return Vec::new();
    };
    // The persisted 1–3 setting now represents groups of ten dramas.
    let groups = number(config, "concurrency", 1).clamp(1, 3) as usize;
    let capacity = groups * GROUP_SIZE;
    let jobs: Vec<_> = snapshot
        .jobs
        .iter()
        .filter(|j| text(&j.config, "channel") == text(config, "channel"))
        .collect();
    let mut occupied = [0usize; 8];
    for job in &jobs {
        if busy.contains(&job.id) {
            occupied[lane(job)] += 1;
        }
    }
    // A group is a rolling production window, not a stage barrier: each drama
    // advances independently and completion frees one slot immediately. Keep
    // retries/reviews in the window so blocked uploads cannot fill the disk.
    // Legacy jobs with files or later stages retain admission without migration.
    let in_window = |j: &Task| {
        !matches!(j.status, Status::Completed | Status::Skipped)
            && (j.download_admitted
                || !j.files.is_empty()
                || !matches!(j.stage.as_str(), "inspect" | "download"))
    };
    let mut buffered = jobs
        .iter()
        .filter(|j| in_window(j) || busy.contains(&j.id) && j.stage == "download")
        .count();
    let mut ready = Vec::new();
    for job in jobs {
        if job.terminal() || job.retry_at > timestamp || busy.contains(&job.id) {
            continue;
        }
        let slot = lane(job);
        // Keep remote preflight and generative API calls bounded separately.
        // Media lanes submit/poll jobs; native CPU/GPU and upload queues still
        // own actual execution admission rather than launching ten encoders.
        let limit = if matches!(slot, 4 | 7) {
            groups
        } else {
            capacity
        };
        if occupied[slot] >= limit {
            continue;
        }
        if job.stage == "download" && !in_window(job) {
            if buffered >= capacity {
                continue;
            }
            buffered += 1;
        }
        occupied[slot] += 1;
        ready.push(job.id.clone());
    }
    ready
}
