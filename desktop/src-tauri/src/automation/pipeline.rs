use super::model::{text, Mode, Snapshot, Status, Task};
use std::collections::HashSet;

pub(super) const GROUP_SIZE: usize = 10;

fn queue_order(left: &Task, right: &Task) -> std::cmp::Ordering {
    left.queue_order
        .cmp(&right.queue_order)
        .then_with(|| left.id.cmp(&right.id))
}

pub(super) fn uses_media_slot(job: &Task) -> bool {
    // A group slot belongs to the whole drama lifecycle. Upload, optional
    // Shorts, and local cleanup must all finish before the next drama enters
    // the ten-item group; releasing it at upload admission lets the scanner
    // overfill the group and interleave unrelated lifecycles.
    !matches!(job.status, Status::Completed | Status::Skipped)
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
    // Backfill records written before v0.4.6 using their persisted array
    // order, then keep the queue sorted after every state transition.
    let missing = snapshot
        .jobs
        .iter()
        .chain(snapshot.waiting.iter())
        .filter(|job| job.queue_order == 0)
        .count() as u64;
    let first_existing = snapshot
        .jobs
        .iter()
        .chain(snapshot.waiting.iter())
        .filter_map(|job| (job.queue_order > 0).then_some(job.queue_order))
        .min();
    if first_existing.is_some_and(|value| value <= missing) {
        for job in snapshot
            .jobs
            .iter_mut()
            .chain(snapshot.waiting.iter_mut())
            .filter(|job| job.queue_order > 0)
        {
            job.queue_order = job.queue_order.saturating_add(missing);
        }
    }
    let mut next = first_existing
        .map(|value| value.saturating_sub(missing).max(1))
        .unwrap_or(1);
    for job in snapshot.jobs.iter_mut().chain(snapshot.waiting.iter_mut()) {
        if job.queue_order == 0 {
            job.queue_order = next;
            next = next.saturating_add(1);
        }
    }
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
    snapshot.jobs.sort_by(queue_order);
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
    let mut jobs: Vec<_> = snapshot
        .jobs
        .iter()
        .filter(|j| text(&j.config, "channel") == text(config, "channel"))
        .collect();
    jobs.sort_by(|left, right| queue_order(left, right));
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
