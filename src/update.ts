// Pure state and presentation logic for the in-app updater. Kept free of any
// Tauri imports so it runs under vitest in node (see update.test.ts); the DOM
// wiring and the actual plugin calls (check / downloadAndInstall / relaunch)
// live in main.ts, which already runs inside the Tauri webview.

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; version: string }
  | { kind: "downloading"; percent: number }
  | { kind: "installing" }
  | { kind: "up-to-date" }
  | { kind: "check-failed" }
  | { kind: "error" };

// Clamp a running byte count to an integer 0–100 percent. Returns 0 when the
// total is unknown (<= 0) so a missing Content-Length can never yield NaN or
// Infinity in the status text.
export function clampPercent(downloaded: number, total: number): number {
  if (total <= 0) return 0;
  const pct = Math.round((downloaded / total) * 100);
  return Math.min(100, Math.max(0, pct));
}

// User-facing label for each state (matches the app UI language: English).
export function statusLabel(state: UpdateState): string {
  switch (state.kind) {
    case "idle":
      return "Check for updates";
    case "checking":
      return "Checking…";
    case "available":
      return "Update available";
    case "downloading":
      return `Downloading ${state.percent}%…`;
    case "installing":
      return "Installing…";
    case "up-to-date":
      return "You're up to date";
    case "check-failed":
      return "Couldn't check for updates";
    case "error":
      return "Update failed";
  }
}

// The status doubles as a button only when the user can start an action: a
// manual check when idle, or the one-click update when one is available. While
// checking, downloading, or installing it must not be re-triggerable.
export function isClickable(state: UpdateState): boolean {
  return state.kind === "idle" || state.kind === "available";
}
