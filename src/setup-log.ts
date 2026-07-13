// A setup download fires a progress event per chunk received (potentially
// thousands for a multi-hundred-MB file). Appending every one as a new line
// grows the log unbounded and floods the DOM. Consecutive progress updates
// (the "X / Y MB" detail Rust emits) replace the previous line instead.
const PROGRESS_DETAIL_RE = /^[\d.]+ \/ [\d.]+ MB$/;

export function appendLogLine(log: string, detail: string): string {
  if (!detail) return log;

  const lines = log.length ? log.split("\n") : [];
  const lastLine = lines[lines.length - 1];

  if (PROGRESS_DETAIL_RE.test(detail) && lastLine !== undefined && PROGRESS_DETAIL_RE.test(lastLine)) {
    lines[lines.length - 1] = detail;
  } else {
    lines.push(detail);
  }

  return lines.join("\n");
}
