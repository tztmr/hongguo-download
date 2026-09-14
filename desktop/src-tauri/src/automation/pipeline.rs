use super::model::{text, Mode, Snapshot, Status, Task};
use std::collections::HashSet;

pub(super) const GROUP_SIZE: usize = 10;

pub(super) fn uses_media_slot(job: &Task) -> bool {
    !matches!(job.status, Status::Completed | Status::Skipped)
        && !matches!(job.stage.as_str(), "upload" | "short" | "cleanup" | "done")
}

pub(super) fn free_slots(snapshot: &Snapshot) -> usize {
    let Some(config) = snapshot.config.as_ref() else {
        return 0;
    };
    GROUP_SIZE.saturating_sub(
        snapshot
            .jobs
            .iter()
            .filter(|job| {
                text(&job.config, "channel") == text(config, "channel") && uses_media_slot(job)
            })
            .count(),
    )
}

pub(super) fn can_scan(snapshot: &Snapshot) -> bool {
    snapshot.mode == Mode::Running && free_slots(snapshot) > 0
}

// Keep at most ten media-stage dramas; uploads have their own network queue.
// Discard only excess unstarted records;
// downloaded files and existing media/upload work must always finish safely.
pub(super) fn normalize_group(snapshot: &mut Snapshot) {
    let Some(config) = snapshot.config.as_ref() else {
        return;
    };
    let channel = text(config, "channel").to_owned();
    let unfinished = |j: &Task| !matches!(j.status, Status::Completed | Status::Skipped);
    let started = |j: &Task| {
        j.download_admitted
            || !j.files.is_empty()
            || !matches!(j.stage.as_str(), "inspect" | "download")
    };
    for task in snapshot.waiting.drain(..) {
        if started(&task) {
            snapshot.jobs.push(task);
        }
    }
    let reserved = snapshot
        .jobs
        .iter()
        .filter(|j| text(&j.config, "channel") == channel && uses_media_slot(j) && started(j))
        .count();
    let mut free = GROUP_SIZE.saturating_sub(reserved);
    snapshot.jobs.retain(|j| {
        if text(&j.config, "channel") != channel || !unfinished(j) || started(j) {
            return true;
        }
        if free > 0 {
            free -= 1;
            true
        } else {
            false
        }
    });
}

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
    let capacity = GROUP_SIZE;
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
    // Members advance independently. The scanner fills only vacant slots.
    let in_window = |j: &Task| {
        uses_media_slot(j)
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
        let limit = if matches!(slot, 4 | 7) { 1 } else { capacity };
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
