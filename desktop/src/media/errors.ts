import type { MediaCommandError } from "./types";

function readCode(record: Record<string, unknown>): MediaCommandError | undefined {
  if (typeof record.code === "string") {
    return {
      code: record.code,
      message: typeof record.message === "string" && record.message ? record.message : record.code,
    };
  }
  for (const key of ["payload", "error", "data"]) {
    const nested = record[key];
    if (nested && typeof nested === "object") {
      const found = readCode(nested as Record<string, unknown>);
      if (found) return found;
    }
  }
  return undefined;
}

export function asMediaError(error: unknown): MediaCommandError {
  if (error && typeof error === "object") {
    const found = readCode(error as Record<string, unknown>);
    if (found) return found;
  }
  if (error instanceof Error && error.message) {
    return { code: "MEDIA_COMMAND_FAILED", message: error.message };
  }
  return { code: "MEDIA_COMMAND_FAILED", message: String(error) };
}
