import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

const inputSelect = document.querySelector<HTMLSelectElement>("#live-device")!;
const outputSelect = document.querySelector<HTMLSelectElement>("#live-output-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#live-status")!;
const statusDot = document.querySelector<HTMLSpanElement>("#status-dot")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
const modeSelector = document.querySelector<HTMLDivElement>("#mode-selector")!;
const pttConfig = document.querySelector<HTMLDivElement>("#ptt-config")!;
const shortcutDisplay = document.querySelector<HTMLSpanElement>("#ptt-shortcut-display")!;
const captureBtn = document.querySelector<HTMLButtonElement>("#btn-capture-shortcut")!;
const recIndicator = document.querySelector<HTMLDivElement>("#ptt-recording-indicator")!;

let listening = false;
let mode: "vad" | "ptt" = "vad";
let pttShortcut: string | null = null;
let isCapturingShortcut = false;

// ── Overlay helper ────────────────────────────────────────────────────────────

async function overlay(): Promise<WebviewWindow | null> {
  return WebviewWindow.getByLabel("overlay");
}

// ── Overlay enable/disable preference ─────────────────────────────────────────

const overlayToggle = document.querySelector<HTMLButtonElement>("#overlay-toggle")!;

function overlayEnabled(): boolean {
  return localStorage.getItem("lt.overlayEnabled") !== "0";
}

function setOverlayChecked(on: boolean): void {
  overlayToggle.setAttribute("aria-checked", on ? "true" : "false");
}

// Reflect the stored preference on load.
setOverlayChecked(overlayEnabled());

overlayToggle.addEventListener("click", async () => {
  const next = !overlayEnabled();
  localStorage.setItem("lt.overlayEnabled", next ? "1" : "0");
  setOverlayChecked(next);
  // Apply live: hide immediately if turned off; show if a session is active.
  if (next) {
    if (listening) void (await overlay())?.show();
  } else {
    void (await overlay())?.hide();
  }
});

// ── Phrase listener ───────────────────────────────────────────────────────────

void listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  (e) => {
    const li = document.createElement("li");
    if (e.payload.error) {
      li.textContent = e.payload.error;
      li.classList.add("error");
    } else {
      const src = document.createElement("span");
      src.style.color = "var(--muted)";
      src.textContent = `ES: ${e.payload.source_text}`;

      const arrow = document.createElement("span");
      arrow.style.color = "var(--accent)";
      arrow.textContent = " → ";

      const tgt = document.createElement("span");
      tgt.style.color = "var(--text)";
      tgt.textContent = `EN: ${e.payload.translated_text}`;

      li.append(src, arrow, tgt);
    }
    phrases.prepend(li);
  },
);

// ── Engine warmup listener ────────────────────────────────────────────────────

void listen("engine-starting", () => {
  statusEl.textContent = "Starting translation engine… (first run may take 1–2 min)";
});

// ── PTT state listener ────────────────────────────────────────────────────────

void listen<boolean>("ptt-state", (e) => {
  recIndicator.classList.toggle("hidden", !e.payload);
});

// ── Mode selector ─────────────────────────────────────────────────────────────

modeSelector.querySelectorAll<HTMLButtonElement>(".mode-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    if (listening) return;
    mode = btn.dataset["mode"] as "vad" | "ptt";
    modeSelector
      .querySelectorAll(".mode-btn")
      .forEach((b) => b.classList.remove("active"));
    btn.classList.add("active");
    pttConfig.classList.toggle("hidden", mode !== "ptt");
  });
});

// ── Shortcut capture ──────────────────────────────────────────────────────────

captureBtn.addEventListener("click", () => {
  isCapturingShortcut = true;
  shortcutDisplay.textContent = "Press a key…";
  shortcutDisplay.classList.add("capturing");
  captureBtn.disabled = true;
});

window.addEventListener("keydown", (e) => {
  if (!isCapturingShortcut) return;
  e.preventDefault();

  const parts: string[] = [];
  if (e.ctrlKey) parts.push("ctrl");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey) parts.push("shift");

  const key = e.key.toLowerCase();
  if (!["control", "alt", "shift", "meta"].includes(key)) {
    parts.push(key === " " ? "space" : key);
  }

  if (parts.length >= 2) {
    const shortcutStr = parts.join("+");
    pttShortcut = shortcutStr;
    shortcutDisplay.textContent = shortcutStr;
    shortcutDisplay.classList.remove("capturing");
    isCapturingShortcut = false;
    captureBtn.disabled = false;
    void invoke("register_ptt_shortcut", { shortcutStr });
  }
});

// ── Device loader ─────────────────────────────────────────────────────────────

async function loadDevices(): Promise<void> {
  const opt = document.createElement("option");
  opt.value = "";
  opt.textContent = "Default microphone";
  inputSelect.replaceChildren(opt);

  const outputs: string[] = await invoke("get_output_devices");
  outputSelect.replaceChildren();
  for (const name of outputs) {
    const o = document.createElement("option");
    o.value = name;
    o.textContent = name;
    outputSelect.appendChild(o);
  }
}

// ── Toggle UI helper ──────────────────────────────────────────────────────────

function setToggle(active: boolean): void {
  const icon = toggle.querySelector<HTMLSpanElement>(".toggle-icon")!;
  const label = toggle.querySelector<HTMLSpanElement>(".toggle-label")!;
  modeSelector
    .querySelectorAll<HTMLButtonElement>(".mode-btn")
    .forEach((b) => (b.disabled = active));

  if (active) {
    icon.textContent = "■";
    label.textContent = "Stop";
    toggle.classList.add("running");
    statusDot.classList.add("active");
    statusEl.textContent = mode === "ptt" ? "PTT ready — press shortcut to record" : "Listening…";
  } else {
    icon.textContent = "▶";
    label.textContent = "Start";
    toggle.classList.remove("running");
    statusDot.classList.remove("active");
    statusEl.textContent = "Idle";
    recIndicator.classList.add("hidden");
  }
}

// ── Start / stop ──────────────────────────────────────────────────────────────

toggle.addEventListener("click", async () => {
  toggle.disabled = true;
  try {
    if (!listening) {
      if (mode === "ptt" && !pttShortcut) {
        statusEl.textContent = "Set a PTT shortcut first";
        return;
      }
      const cmd =
        mode === "ptt" ? "start_live_translation_ptt" : "start_live_translation";
      await invoke(cmd, {
        deviceName: inputSelect.value,
        outputDeviceName: outputSelect.value,
      });
      listening = true;
      setToggle(true);
      if (overlayEnabled()) void (await overlay())?.show();
    } else {
      await invoke("stop_live_translation");
      listening = false;
      setToggle(false);
      void (await overlay())?.hide();
    }
  } catch (err) {
    statusEl.textContent = `Error: ${err}`;
  } finally {
    toggle.disabled = false;
  }
});

void loadDevices();
