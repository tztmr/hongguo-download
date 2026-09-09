import { sameFsPath } from "../media/paths";
import type { MediaJob } from "../media/types";

export type UploadVideoSource = { kind: "merged" | "noBackgroundMusic"; path: string };

export function getUploadSourceOptions(sourcePath: string, jobs: MediaJob[], knownMergedPath?: string): UploadVideoSource[] {
  const separations = jobs.slice().reverse().filter((job) => job.kind === "separateBackgroundMusic"
    && job.status === "completed" && job.aiRequest?.scope === "merged");
  const cleanPath = (job?: MediaJob) => job?.outputs?.find((output) => output.kind === "noBackgroundMusicVideo")?.path;
  const selectedSeparation = separations.find((job) => sameFsPath(cleanPath(job), sourcePath));
  const usesInput = (job: MediaJob, path?: string | null) => Boolean(path) && job.inputs.some((input) => sameFsPath(input.path, path));
  const original = selectedSeparation
    ? jobs.find((job) => job.kind === "merge" && job.status === "completed" && usesInput(selectedSeparation, job.outputPath))?.outputPath
      || (usesInput(selectedSeparation, knownMergedPath) ? knownMergedPath : undefined)
    : sourcePath;
  const cleaned = selectedSeparation ? sourcePath : cleanPath(separations.find((job) => usesInput(job, original)));
  const options: UploadVideoSource[] = [];
  if (original) options.push({ kind: "merged", path: original });
  if (cleaned && !sameFsPath(original, cleaned)) options.push({ kind: "noBackgroundMusic", path: cleaned });
  return options;
}

export function availableUploadSources(sourcePath: string, options?: UploadVideoSource[]): UploadVideoSource[] {
  return options?.length ? options : [{ kind: "merged", path: sourcePath }];
}
