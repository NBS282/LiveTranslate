import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

// ── Helpers ──────────────────────────────────────────────────────────────────

function show(id: string): void {
  document.querySelectorAll(".screen").forEach((el) => el.classList.add("hidden"));
  document.getElementById(id)?.classList.remove("hidden");
}

function setCheckIcon(id: string, ok: boolean): void {
  const icon = document.getElementById(id)?.querySelector<HTMLSpanElement>(".check-icon");
  if (icon) icon.dataset.state = ok ? "ok" : "idle";
}

function setProgress(
  fillId: string,
  pctId: string,
  stepId: string,
  pct: number,
  step: string,
): void {
  const fill = document.getElementById(fillId);
  const pctEl = document.getElementById(pctId);
  const stepEl = document.getElementById(stepId);
  if (fill) fill.style.width = `${pct}%`;
  if (pctEl) pctEl.textContent = `${pct}%`;
  if (stepEl) stepEl.textContent = step;
}

function setStatusBadge(id: string, state: "ok" | "err" | "idle", text: string): void {
  const el = document.getElementById(id);
  if (!el) return;
  el.classList.remove("ok", "err");
  if (state !== "idle") el.classList.add(state);
  const badge = el.querySelector<HTMLSpanElement>(".badge-text");
  if (badge) badge.textContent = text;
}

function setStepDot(active: 1 | 2 | 3): void {
  document.querySelectorAll<HTMLElement>(".step-dot").forEach((dot, i) => {
    dot.classList.remove("active", "done");
    const n = i + 1;
    if (n < active) dot.classList.add("done");
    else if (n === active) dot.classList.add("active");
  });
}

// ── Global setup-progress listener (setup screen only) ───────────────────────

const setupLog = document.getElementById("setup-log")!;

listen<{ step: string; percent: number; detail: string }>("setup-progress", (e) => {
  const { step, percent, detail } = e.payload;
  setProgress("setup-bar-fill", "setup-pct", "setup-step", percent, step);
  if (detail) {
    setupLog.textContent += `${detail}\n`;
    setupLog.scrollTop = setupLog.scrollHeight;
  }
});

// ── Init ─────────────────────────────────────────────────────────────────────

async function init(): Promise<void> {
  try {
    const status = await invoke<{ venv_ok: boolean; piper_voice_ok: boolean; ready: boolean }>(
      "check_setup",
    );
    setCheckIcon("setup-check-venv", status.venv_ok);
    setCheckIcon("setup-check-piper", status.piper_voice_ok);

    if (status.ready) {
      show("screen-onboarding");
      setStepDot(1);
      void checkVBCable();
      void checkPiper();
    } else {
      show("screen-setup");
    }
  } catch (err) {
    console.error("init failed:", err);
    show("screen-setup");
  }
}

// ── Setup screen ─────────────────────────────────────────────────────────────

const btnRunSetup = document.getElementById("btn-run-setup") as HTMLButtonElement;

btnRunSetup.addEventListener("click", () => {
  btnRunSetup.disabled = true;
  btnRunSetup.textContent = "Installing…";
  document.getElementById("setup-progress-box")!.classList.remove("hidden");
  setupLog.textContent = "";
  void invoke("start_setup");
});

listen<{ success: boolean; error?: string }>("setup-done", (e) => {
  if (e.payload.success) {
    setCheckIcon("setup-check-venv", true);
    setCheckIcon("setup-check-piper", true);
    show("screen-onboarding");
    setStepDot(1);
    void checkVBCable();
    void checkPiper();
  } else {
    document.getElementById("setup-step")!.textContent = `Error: ${e.payload.error ?? "unknown"}`;
    btnRunSetup.disabled = false;
    btnRunSetup.textContent = "Retry";
  }
});

// ── Onboarding step 1: VB-Cable ──────────────────────────────────────────────

const btnRecheck = document.getElementById("btn-ob-cable-check") as HTMLButtonElement;

async function checkVBCable(): Promise<void> {
  btnRecheck.disabled = true;
  btnRecheck.textContent = "Checking…";

  try {
    const found = await invoke<boolean>("check_vbcable");
    if (found) {
      setStatusBadge("ob-cable-status", "ok", "VB-Cable detected");
      document.getElementById("ob-cable-install")!.classList.add("hidden");
      document.getElementById("btn-ob-next-1")!.classList.remove("hidden");
    } else {
      setStatusBadge("ob-cable-status", "err", "Not found — install VB-Cable then click Re-check");
      document.getElementById("ob-cable-install")!.classList.remove("hidden");
      document.getElementById("btn-ob-next-1")!.classList.add("hidden");
    }
  } catch (err) {
    setStatusBadge("ob-cable-status", "err", `Check failed: ${err}`);
  } finally {
    btnRecheck.disabled = false;
    btnRecheck.textContent = "Re-check";
  }
}

btnRecheck.addEventListener("click", () => void checkVBCable());

document.getElementById("ob-cable-install")!.addEventListener("click", () => {
  void openUrl("https://vb-audio.com/Cable/");
});

document.getElementById("btn-ob-next-1")!.addEventListener("click", () => {
  document.getElementById("ob-step-1")!.classList.add("hidden");
  document.getElementById("ob-step-2")!.classList.remove("hidden");
  setStepDot(2);
  void checkPiper();
});

// ── Onboarding step 2: Piper voice ───────────────────────────────────────────

async function checkPiper(): Promise<void> {
  try {
    const st = await invoke<{ venv_ok: boolean; piper_voice_ok: boolean; ready: boolean }>(
      "check_setup",
    );
    if (st.piper_voice_ok) {
      setStatusBadge("ob-piper-status", "ok", "Voice model ready");
      document.getElementById("btn-ob-dl-piper")!.classList.add("hidden");
      document.getElementById("btn-ob-next-2")!.classList.remove("hidden");
    } else {
      setStatusBadge("ob-piper-status", "idle", "Not downloaded yet");
      (document.getElementById("btn-ob-dl-piper") as HTMLButtonElement).disabled = false;
      document.getElementById("btn-ob-dl-piper")!.classList.remove("hidden");
      document.getElementById("btn-ob-next-2")!.classList.add("hidden");
    }
  } catch (err) {
    setStatusBadge("ob-piper-status", "err", `Check failed: ${err}`);
  }
}

const btnDlPiper = document.getElementById("btn-ob-dl-piper") as HTMLButtonElement;

btnDlPiper.addEventListener("click", async () => {
  btnDlPiper.disabled = true;
  btnDlPiper.textContent = "Downloading…";
  document.getElementById("ob-piper-progress")!.classList.remove("hidden");

  const unlisten = await listen<{ step: string; percent: number; detail: string }>(
    "setup-progress",
    (e) => {
      setProgress("ob-piper-fill", "ob-piper-pct", "ob-piper-step", e.payload.percent, e.payload.step);
    },
  );

  try {
    await invoke("download_piper_voice");
    setStatusBadge("ob-piper-status", "ok", "Voice model ready");
    document.getElementById("btn-ob-next-2")!.classList.remove("hidden");
    btnDlPiper.classList.add("hidden");
  } catch (err) {
    setStatusBadge("ob-piper-status", "err", `Download failed: ${err}`);
    btnDlPiper.disabled = false;
    btnDlPiper.textContent = "Retry";
  } finally {
    unlisten();
  }
});

document.getElementById("btn-ob-next-2")!.addEventListener("click", () => {
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.remove("hidden");
  setStepDot(3);
});

// ── Onboarding step 3 ────────────────────────────────────────────────────────

document.getElementById("btn-ob-finish")!.addEventListener("click", () => {
  show("screen-main");
  void import("./live.ts");
});

// ── Settings from main screen ─────────────────────────────────────────────────

document.getElementById("btn-reopen-onboarding")!.addEventListener("click", () => {
  show("screen-onboarding");
  setStepDot(1);

  document.getElementById("ob-step-1")!.classList.remove("hidden");
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.add("hidden");

  document.getElementById("btn-ob-next-1")!.classList.add("hidden");
  document.getElementById("ob-cable-install")!.classList.add("hidden");
  document.getElementById("ob-piper-progress")!.classList.add("hidden");
  document.getElementById("btn-ob-next-2")!.classList.add("hidden");
  btnDlPiper.classList.remove("hidden");
  btnDlPiper.disabled = false;
  btnDlPiper.textContent = "Download voice model";

  setStatusBadge("ob-cable-status", "idle", "Checking…");
  setStatusBadge("ob-piper-status", "idle", "Checking…");

  void checkVBCable();
  void checkPiper();
});

// ── Boot ──────────────────────────────────────────────────────────────────────

void init();
