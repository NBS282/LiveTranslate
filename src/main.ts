import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { appendLogLine } from "./setup-log";
import { clampPercent, isClickable, statusLabel, type UpdateState } from "./update";

// ── Helpers ──────────────────────────────────────────────────────────────────

// Minimum splash display time so the user sees it.
const SPLASH_MIN_MS = 1200;
const splashStart = performance.now();

function show(id: string): void {
  document.querySelectorAll(".screen").forEach((el) => el.classList.add("hidden"));
  document.getElementById(id)?.classList.remove("hidden");
  // Hide splash once a screen transitions in, but keep it visible for
  // at least SPLASH_MIN_MS so the user perceives the loading state.
  const splash = document.getElementById("splash");
  if (splash) {
    const elapsed = performance.now() - splashStart;
    const delay = Math.max(0, SPLASH_MIN_MS - elapsed);
    if (delay > 0) {
      setTimeout(() => splash.classList.add("hidden"), delay);
    } else {
      splash.classList.add("hidden");
    }
  }
}

function friendlySetupText(step: string): string {
  const s = step.toLowerCase();
  if (s.includes("python") || s.includes("environment") || s.includes("pip") || s.includes("extract"))
    return "Getting everything ready…";
  if (s.includes("pytorch") || s.includes("engine") || s.includes("packages"))
    return "Installing the translation engine…";
  if (s.includes("voice") || s.includes("piper") || s.includes("model"))
    return "Downloading the voice…";
  if (s.includes("complete"))
    return "Almost done…";
  return "Getting everything ready…";
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

function setStepDot(active: 1 | 2 | 3 | 4): void {
  document.querySelectorAll<HTMLElement>(".step-dot").forEach((dot, i) => {
    dot.classList.remove("active", "done");
    const n = i + 1;
    if (n < active) dot.classList.add("done");
    else if (n === active) dot.classList.add("active");
  });
}

// ── Legacy setup log element reference ────────────────────────────────────────

const setupLog = document.getElementById("setup-log")!;

// ── Init ─────────────────────────────────────────────────────────────────────

async function init(): Promise<void> {
  try {
    const status = await invoke<{ venv_ok: boolean; piper_voice_ok: boolean; ready: boolean }>(
      "check_setup",
    );

    // Already installed and onboarded → go straight to the app.
    if (status.ready && localStorage.getItem("lt.onboarded") === "1") {
      show("screen-main");
      void import("./live.ts");
      return;
    }

    if (!status.ready) {
      // Not installed yet → mandatory install screen (user must complete this first).
      show("screen-setup");
      return;
    }

    // Installed but onboarding not yet finished (e.g. restarted mid-onboarding).
    // Warm the engine in the background so models load while the user onboards.
    void invoke("warm_engine");
    show("screen-onboarding");
    setStepDot(1);
    void checkVBCable();
  } catch (err) {
    console.error("init failed:", err);
    show("screen-setup");
  }
}

// ── Onboarding: setup installation ───────────────────────────────────────────

document.getElementById("btn-ob-setup")!.addEventListener("click", () => {
  const btn = document.getElementById("btn-ob-setup") as HTMLButtonElement;
  btn.disabled = true;
  btn.textContent = "Installing…";
  document.getElementById("ob-setup-progress")!.classList.remove("hidden");
  void invoke("start_setup");
});

// ── Shared setup-progress listener (works for onboarding prompt & legacy screen) ──

let setupOrigin: "onboarding" | "screen-setup" = "onboarding";

listen<{ step: string; percent: number; detail: string }>("setup-progress", (e) => {
  const { step, percent, detail } = e.payload;
  if (setupOrigin === "onboarding") {
    setProgress("ob-setup-bar-fill", "ob-setup-pct", "ob-setup-step", percent, friendlySetupText(step));
  } else {
    setProgress("setup-bar-fill", "setup-pct", "setup-step", percent, friendlySetupText(step));
    if (detail) {
      setupLog.textContent = appendLogLine(setupLog.textContent ?? "", detail);
      setupLog.scrollTop = setupLog.scrollHeight;
    }
  }
});

listen<{ success: boolean; error?: string }>("setup-done", (e) => {
  if (e.payload.success) {
    // Setup just finished — warm the engine NOW so the Python sidecar
    // (Marian MT + Piper/Pocket TTS, and NeMo Parakeet ASR only when
    // LT_STT_BACKEND=python) loads while the user goes through onboarding.
    // By the time they hit "Start", the server is ready and translation
    // works on the first try. The native STT model (GGUF Parakeet) loads
    // separately, on the first live-translation session.
    void invoke("warm_engine");

    // Hide setup prompt in onboarding, start VB-Cable check
    document.getElementById("ob-setup-prompt")!.classList.add("hidden");
    document.getElementById("btn-ob-next-1")!.classList.remove("hidden");
    show("screen-onboarding");
    setStepDot(1);
    void checkVBCable();

    // Reset origin for next time
    setupOrigin = "onboarding";
  } else {
    // Show error where appropriate
    if (setupOrigin === "onboarding") {
      document.getElementById("ob-setup-step")!.textContent = "Something went wrong. Try again.";
      const btn = document.getElementById("btn-ob-setup") as HTMLButtonElement;
      btn.disabled = false;
      btn.textContent = "Retry";
    } else {
      const errMsg = e.payload.error ?? "";
      const firstLine = errMsg.split("\n")[0] || "Something went wrong. Check the details and try again.";
      document.getElementById("setup-step")!.textContent = firstLine;
      const log = document.getElementById("setup-log")!;
      if (errMsg) {
        log.textContent += `\n--- Error ---\n${errMsg}\n`;
        log.scrollTop = log.scrollHeight;
      }
      log.classList.remove("hidden");
      document.getElementById("setup-details-toggle")!.textContent = "Hide details";
      const btn = document.getElementById("btn-run-setup") as HTMLButtonElement;
      btn.disabled = false;
      btn.textContent = "Retry";
    }
  }
});

// ── Legacy setup screen (still reachable, sets origin so listeners work) ─────

document.getElementById("btn-run-setup")!.addEventListener("click", () => {
  setupOrigin = "screen-setup";
  const btn = document.getElementById("btn-run-setup") as HTMLButtonElement;
  btn.disabled = true;
  btn.textContent = "Installing…";
  document.getElementById("setup-progress-box")!.classList.remove("hidden");
  setupLog.textContent = "";
  void invoke("start_setup");
});

document.getElementById("setup-details-toggle")!.addEventListener("click", () => {
  const log = document.getElementById("setup-log")!;
  const hidden = log.classList.toggle("hidden");
  document.getElementById("setup-details-toggle")!.textContent = hidden ? "View details" : "Hide details";
});

// ── Onboarding step 1: virtual audio device (VB-Cable / BlackHole) ───────────

const btnRecheck = document.getElementById("btn-ob-cable-check") as HTMLButtonElement;

// Windows defaults; swapped for macOS once the platform is known.
let virtualCable = { name: "VB-Cable", url: "https://vb-audio.com/Cable/" };

const platformReady: Promise<void> = (async () => {
  try {
    const os = await invoke<string>("get_platform");
    if (os === "macos") {
      virtualCable = { name: "BlackHole", url: "https://existential.audio/blackhole/" };
      document.getElementById("ob-cable-name")!.textContent = virtualCable.name;
      document.getElementById("ob-cable-dl-label")!.textContent = `Download ${virtualCable.name}.`;
    }
  } catch {
    // Unknown platform: keep the Windows copy.
  }
})();

async function checkVBCable(): Promise<void> {
  btnRecheck.disabled = true;
  btnRecheck.textContent = "Checking…";

  try {
    await platformReady;
    const found = await invoke<boolean>("check_vbcable");
    if (found) {
      setStatusBadge("ob-cable-status", "ok", `${virtualCable.name} detected`);
      document.getElementById("ob-cable-install")!.classList.add("hidden");
      document.getElementById("btn-ob-next-1")!.classList.remove("hidden");
    } else {
      setStatusBadge("ob-cable-status", "err", `Not found — install ${virtualCable.name} and check again`);
      document.getElementById("ob-cable-install")!.classList.remove("hidden");
      document.getElementById("btn-ob-next-1")!.classList.add("hidden");
    }
  } catch (err) {
    setStatusBadge("ob-cable-status", "err", `Check failed: ${err}`);
  } finally {
    btnRecheck.disabled = false;
    btnRecheck.textContent = "Check again";
  }
}

btnRecheck.addEventListener("click", () => void checkVBCable());

document.getElementById("ob-cable-install")!.addEventListener("click", () => {
  void openUrl(virtualCable.url);
});

document.getElementById("btn-ob-next-1")!.addEventListener("click", () => {
  document.getElementById("ob-step-1")!.classList.add("hidden");
  document.getElementById("ob-step-2")!.classList.remove("hidden");
  setStepDot(2);
});

// ── Onboarding step 2: connect audio ─────────────────────────────────────────

document.getElementById("btn-ob-next-2")!.addEventListener("click", () => {
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.remove("hidden");
  setStepDot(3);
});

// ── Onboarding step 3: shortcuts ─────────────────────────────────────────────

document.getElementById("btn-ob-next-3")!.addEventListener("click", () => {
  document.getElementById("ob-step-3")!.classList.add("hidden");
  document.getElementById("ob-step-4")!.classList.remove("hidden");
  setStepDot(4);
});

// ── Onboarding step 4: finish ────────────────────────────────────────────────

document.getElementById("btn-ob-finish")!.addEventListener("click", () => {
  localStorage.setItem("lt.onboarded", "1");
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
  document.getElementById("ob-step-4")!.classList.add("hidden");

  document.getElementById("btn-ob-next-1")!.classList.add("hidden");
  document.getElementById("ob-cable-install")!.classList.add("hidden");

  setStatusBadge("ob-cable-status", "idle", "Verificando…");
  void checkVBCable();
});

// ── In-app updater (Handy-style footer status) ───────────────────────────────
//
// A single footer button in the main screen. It auto-checks once, silently, on
// boot. Clicking it runs a manual check when idle, or — when an update is
// available — downloads, installs, and relaunches in one click. All Tauri calls
// live here; the pure state/label/percent logic is in update.ts (unit-tested).

function setupUpdater(): void {
  const el = document.getElementById("update-status") as HTMLButtonElement | null;
  if (!el) return;

  let current: UpdateState = { kind: "idle" };
  let revertTimer: number | undefined;

  function render(state: UpdateState): void {
    current = state;
    el!.textContent = statusLabel(state);
    el!.disabled = !isClickable(state);
    el!.classList.toggle("is-available", state.kind === "available");
  }

  async function runCheck(manual: boolean): Promise<void> {
    if (revertTimer !== undefined) {
      clearTimeout(revertTimer);
      revertTimer = undefined;
    }
    render({ kind: "checking" });
    try {
      const update = await check();
      if (update) {
        render({ kind: "available", version: update.version });
      } else if (manual) {
        // Only acknowledge "up to date" after a user-initiated check, then
        // fall back to the idle affordance a few seconds later.
        render({ kind: "up-to-date" });
        revertTimer = window.setTimeout(() => render({ kind: "idle" }), 3000);
      } else {
        render({ kind: "idle" });
      }
    } catch (err) {
      // A check throws when the release manifest can't be fetched — e.g. no
      // published latest.json yet, or the machine is offline. Surface it only
      // for a user-initiated check (transient), and stay silent on auto-check.
      console.error("update check failed:", err);
      if (manual) {
        render({ kind: "check-failed" });
        revertTimer = window.setTimeout(() => render({ kind: "idle" }), 3000);
      } else {
        render({ kind: "idle" });
      }
    }
  }

  async function runInstall(): Promise<void> {
    render({ kind: "downloading", percent: 0 });
    try {
      const update = await check();
      if (!update) {
        render({ kind: "idle" });
        return;
      }
      let downloaded = 0;
      let total = 0;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            downloaded = 0;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            render({ kind: "downloading", percent: clampPercent(downloaded, total) });
            break;
          case "Finished":
            render({ kind: "installing" });
            break;
        }
      });
      await relaunch();
    } catch (err) {
      console.error("update install failed:", err);
      render({ kind: "error" });
    }
  }

  el.addEventListener("click", () => {
    if (current.kind === "idle") void runCheck(true);
    else if (current.kind === "available") void runInstall();
  });

  // Silent auto-check on boot.
  void runCheck(false);
}

// ── Boot ──────────────────────────────────────────────────────────────────────

void init();
setupUpdater();
