export type SavePhase = "idle" | "dirty" | "saving" | "saved" | "error";

export function SaveStatus({ phase, children }: { phase: SavePhase; children: React.ReactNode }) {
  return <p className={`save-feedback save-feedback-${phase}`} role="status" aria-live="polite" aria-atomic="true">
    <span className="save-feedback-dot" aria-hidden="true" />{children}
  </p>;
}
