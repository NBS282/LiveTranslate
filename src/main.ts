import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

// ── Helpers ──────────────────────────────────────────────────────────────────

function show(id: string): void {
  document.querySelectorAll(".screen").forEach((el) => el.classList.add("hidden"));
  document.getElementById(id)?.classList.remove("hidden");
}

function friendlySetupText(step: string): string {
  const s = step.toLowerCase();
  if (s.includes("python") || s.includes("environment") || s.includes("pip") || s.includes("extract"))
    return "Preparando todo para ti…";
  if (s.includes("pytorch") || s.includes("engine") || s.includes("packages"))
    return "Instalando el motor de traducción…";
  if (s.includes("voice") || s.includes("piper") || s.includes("model"))
    return "Descargando la voz…";
  if (s.includes("complete"))
    return "Casi listo…";
  return "Preparando todo para ti…";
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

// ── Global setup-progress listener (setup screen only) ───────────────────────

const setupLog = document.getElementById("setup-log")!;

listen<{ step: string; percent: number; detail: string }>("setup-progress", (e) => {
  const { step, percent, detail } = e.payload;
  setProgress("setup-bar-fill", "setup-pct", "setup-step", percent, friendlySetupText(step));
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
    if (status.ready) {
      show("screen-onboarding");
      setStepDot(1);
      void checkVBCable();
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
  btnRunSetup.textContent = "Instalando…";
  document.getElementById("setup-progress-box")!.classList.remove("hidden");
  setupLog.textContent = "";
  void invoke("start_setup");
});

document.getElementById("setup-details-toggle")!.addEventListener("click", () => {
  const log = document.getElementById("setup-log")!;
  const hidden = log.classList.toggle("hidden");
  document.getElementById("setup-details-toggle")!.textContent = hidden ? "Ver detalles" : "Ocultar detalles";
});

listen<{ success: boolean; error?: string }>("setup-done", (e) => {
  if (e.payload.success) {
    show("screen-onboarding");
    setStepDot(1);
    void checkVBCable();
  } else {
    document.getElementById("setup-step")!.textContent = "Algo salió mal. Mirá los detalles e intentá de nuevo.";
    document.getElementById("setup-log")!.classList.remove("hidden");
    document.getElementById("setup-details-toggle")!.textContent = "Ocultar detalles";
    btnRunSetup.disabled = false;
    btnRunSetup.textContent = "Reintentar";
  }
});

// ── Onboarding step 1: VB-Cable ──────────────────────────────────────────────

const btnRecheck = document.getElementById("btn-ob-cable-check") as HTMLButtonElement;

async function checkVBCable(): Promise<void> {
  btnRecheck.disabled = true;
  btnRecheck.textContent = "Verificando…";

  try {
    const found = await invoke<boolean>("check_vbcable");
    if (found) {
      setStatusBadge("ob-cable-status", "ok", "VB-Cable detectado");
      document.getElementById("ob-cable-install")!.classList.add("hidden");
      document.getElementById("btn-ob-next-1")!.classList.remove("hidden");
    } else {
      setStatusBadge("ob-cable-status", "err", "No encontrado — instalá VB-Cable y volvé a verificar");
      document.getElementById("ob-cable-install")!.classList.remove("hidden");
      document.getElementById("btn-ob-next-1")!.classList.add("hidden");
    }
  } catch (err) {
    setStatusBadge("ob-cable-status", "err", `Check failed: ${err}`);
  } finally {
    btnRecheck.disabled = false;
    btnRecheck.textContent = "Re-verificar";
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

// ── Boot ──────────────────────────────────────────────────────────────────────

void init();
