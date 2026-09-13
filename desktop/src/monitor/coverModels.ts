export const COVER_MODEL_CHOICES = ["gpt-image-2", "gpt-image-2-medium", "gpt-image-2.5-flare", "gpt-image-2.5-sunburst"];
export function orderedCoverModels(value: unknown, legacy: string): string[] {
  const models = Array.isArray(value) ? value.filter((v): v is string => typeof v === "string" && /^[\w./:-]{1,200}$/.test(v.trim())).map(v => v.trim()) : [];
  return [...new Set(models.length ? models : legacy.trim() ? [legacy.trim()] : [])].slice(0, 4);
}
export function canTryNextCoverModel(error: unknown): boolean {
  // Only a definite rejection permits a new billable request. A timeout or
  // transport failure may already have generated an image at the provider.
  return !!error && typeof error === "object" && "code" in error &&
    (error.code === "AI_UPSTREAM_ERROR" || error.code === "AI_INVALID_IMAGE");
}
