import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Screen management ──
function show(id: string) {
  document.querySelectorAll(".screen").forEach((el) => el.classList.add("hidden"));
  const el = document.getElementById(id);
  if (el) el.classList.remove("hidden");
}

// ── Shared progress listener (updates hidden setup bar when visible) ──
const setupStep = document.getElementById("setup-step")!;
const setupBar = document.getElementById("setup-bar") as HTMLProgressElement;

listen<{ step: string; percent: number; detail: string }>("setup-progress", (e) => {
  setupStep.textContent = e.payload.step;
  setupBar.value = e.payload.percent;
});

// ── Init ──
async function init() {
  const status = await invoke<{
    venv_ok: boolean;
    piper_voice_ok: boolean;
    ready: boolean;
  }>("check_setup");

  document.getElementById("setup-check-venv")!.textContent = status.venv_ok
    ? "✅ Python environment (.venv-engine)"
    : "⬜ Python environment (.venv-engine)";
  document.getElementById("setup-check-piper")!.textContent = status.piper_voice_ok
    ? "✅ Piper voice model"
    : "⬜ Piper voice model";

  if (status.ready) {
    show("screen-onboarding");
    checkVBCable();
    checkPiper();
  } else {
    show("screen-setup");
  }
}

// ── Setup screen ──
const setupLog = document.getElementById("setup-log")!;

document.getElementById("btn-run-setup")!.addEventListener("click", () => {
  document.getElementById("btn-run-setup")!.setAttribute("disabled", "true");
  document.getElementById("setup-progress-box")!.classList.remove("hidden");
  setupLog.textContent = "";
  invoke("start_setup");
});

listen<{ success: boolean; error?: string }>("setup-done", (e) => {
  if (e.payload.success) {
    document.getElementById("setup-check-venv")!.textContent =
      "✅ Python environment (.venv-engine)";
    document.getElementById("setup-check-piper")!.textContent =
      "✅ Piper voice model";
    show("screen-onboarding");
    checkVBCable();
    checkPiper();
  } else {
    setupStep.textContent = `Error: ${e.payload.error}`;
    document.getElementById("btn-run-setup")!.removeAttribute("disabled");
  }
});

// ── Onboarding step 1: VB-Cable ──
async function checkVBCable() {
  const status = document.getElementById("ob-cable-status")!;
  const link = document.getElementById("ob-cable-link")!;
  const next = document.getElementById("btn-ob-next-1")!;

  const hasCable = await invoke<boolean>("check_vbcable");
  if (hasCable) {
    status.textContent = "✅ VB-Cable detected";
    link.classList.add("hidden");
    next.classList.remove("hidden");
  } else {
    status.textContent = "❌ VB-Cable not found — download and install it, then click Re-check";
    link.classList.remove("hidden");
    next.classList.add("hidden");
  }
}

document.getElementById("btn-ob-cable-check")!.addEventListener("click", checkVBCable);

document.getElementById("btn-ob-next-1")!.addEventListener("click", () => {
  document.getElementById("ob-step-1")!.classList.add("hidden");
  document.getElementById("ob-step-2")!.classList.remove("hidden");
  checkPiper();
});

// ── Onboarding step 2: Piper voice ──
async function checkPiper() {
  const status = document.getElementById("ob-piper-status")!;
  const btnDl = document.getElementById("btn-ob-dl-piper")!;
  const progress = document.getElementById("ob-piper-progress")!;
  const next = document.getElementById("btn-ob-next-2")!;

  const st = await invoke<{
    venv_ok: boolean;
    piper_voice_ok: boolean;
    ready: boolean;
  }>("check_setup");

  // Reset UI
  progress.classList.add("hidden");
  btnDl.classList.remove("hidden");
  btnDl.removeAttribute("disabled");
  next.classList.add("hidden");

  if (st.piper_voice_ok) {
    status.textContent = "✅ Voice model ready";
    btnDl.classList.add("hidden");
    next.classList.remove("hidden");
  } else {
    status.textContent = "⚠ Voice model not downloaded";
  }
}

document.getElementById("btn-ob-dl-piper")!.addEventListener("click", async () => {
  const btn = document.getElementById("btn-ob-dl-piper")!;
  const progress = document.getElementById("ob-piper-progress")!;
  const bar = document.getElementById("ob-piper-bar") as HTMLProgressElement;
  const detail = document.getElementById("ob-piper-detail")!;

  btn.setAttribute("disabled", "true");
  progress.classList.remove("hidden");

  const unlisten = await listen<{
    step: string;
    percent: number;
    detail: string;
  }>("setup-progress", (e) => {
    bar.value = e.payload.percent;
    detail.textContent = e.payload.detail;
  });

  try {
    await invoke("download_piper_voice");
    document.getElementById("ob-piper-status")!.textContent = "✅ Voice model ready";
    document.getElementById("btn-ob-next-2")!.classList.remove("hidden");
  } catch (e) {
    detail.textContent = `Download failed: ${e}`;
    btn.removeAttribute("disabled");
  } finally {
    unlisten();
  }
});

document.getElementById("btn-ob-next-2")!.addEventListener("click", () => {
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.remove("hidden");
});

// ── Onboarding step 3 ──
document.getElementById("btn-ob-finish")!.addEventListener("click", () => {
  show("screen-main");
  import("./live.ts");
});

// ── Re-open onboarding from main ──
document.getElementById("btn-reopen-onboarding")!.addEventListener("click", () => {
  show("screen-onboarding");

  // Reset all steps to initial state
  document.querySelectorAll(".ob-step").forEach((el, i) => {
    if (i === 0) el.classList.remove("hidden");
    else el.classList.add("hidden");
  });

  // Reset dynamic button/indicator visibility
  document.getElementById("btn-ob-next-1")!.classList.add("hidden");
  document.getElementById("btn-ob-next-2")!.classList.add("hidden");
  document.getElementById("ob-cable-link")!.classList.add("hidden");
  document.getElementById("ob-piper-progress")!.classList.add("hidden");
  document.getElementById("ob-step-1")!.classList.remove("hidden");
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.add("hidden");

  document.getElementById("ob-cable-status")!.textContent = "Checking…";
  document.getElementById("ob-piper-status")!.textContent = "Checking voice model…";

  checkVBCable();
  checkPiper();
});

// ── Start ──
init();
