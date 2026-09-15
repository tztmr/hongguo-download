function readErrorMessage(error: unknown, seen = new Set<unknown>()): string | undefined {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  if (typeof error === "number" || typeof error === "boolean") return String(error);
  if (!error || typeof error !== "object" || seen.has(error)) return undefined;
  seen.add(error);
  const record = error as Record<string, unknown>;
  if (typeof record.message === "string" && record.message) return record.message;
  for (const key of ["payload", "error", "data"]) {
    const nested = readErrorMessage(record[key], seen);
    if (nested) return nested;
  }
  return undefined;
}

export function errorMessage(error: unknown) {
  return readErrorMessage(error) || "操作失败，请重试";
}
