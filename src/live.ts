import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { cloneEnabled } from "./voice";
import { pickDefaultOutputDevice } from "./output-device";

const inputSelect = document.querySelector<HTMLSelectElement>("#live-device")!;
const outputSelect = document.querySelector<HTMLSelectElement>("#live-output-device")!;
const langPairSelect = document.querySelector<HTMLSelectElement>("#live-lang-pair")!;
const langPill = document.querySelector<HTMLSpanElement>("#lang-pill")!;
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

// ── Language pair ─────────────────────────────────────────────────────────────
// Canary 1B Flash supports EN<->DE/ES/FR; the <select> only offers those.
// The pair is locked while a session runs — it is captured at start time.

function selectedLangPair(): { src: string; tgt: string } {
  const [src, tgt] = (langPairSelect.value || "es|en").split("|");
  return { src: src || "es", tgt: tgt || "en" };
}

function refreshLangPill(): void {
  const { src, tgt } = selectedLangPair();
  langPill.replaceChildren(
    document.createTextNode(`${src.toUpperCase()} `),
    Object.assign(document.createElement("span"), {
      className: "arrow",
      textContent: "→",
    }),
    document.createTextNode(` ${tgt.toUpperCase()}`),
  );
}

const savedLangPair = localStorage.getItem("lt.langPair");
if (savedLangPair && [...langPairSelect.options].some((o) => o.value === savedLangPair)) {
  langPairSelect.value = savedLangPair;
}
refreshLangPill();

langPairSelect.addEventListener("change", () => {
  localStorage.setItem("lt.langPair", langPairSelect.value);
  refreshLangPill();
});

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

// Reflect the stored preference on load and tell the overlay window about it.
setOverlayChecked(overlayEnabled());
void emit("overlay-toggle", overlayEnabled());

overlayToggle.addEventListener("click", async () => {
  const next = !overlayEnabled();
  localStorage.setItem("lt.overlayEnabled", next ? "1" : "0");
  setOverlayChecked(next);
  // The overlay window owns its own visibility — tell it the new state.
  void emit("overlay-toggle", next);
  // Re-enabling mid-session: show right away so it's ready for the next phrase.
  if (next && listening) void (await overlay())?.show();
});

// ── Partial (in-progress) translation listener ────────────────────────────────
// The engine streams a rough, dimmed preview of the current segment while it is
// still open (Task 7's `partial` event). It stays pinned above the finalized
// phrases and is cleared once the segment lands (a `phrase` event) or closes
// (an empty `partial`).

const partialEl = document.createElement("li");
partialEl.className = "subtitle-partial hidden";
phrases.prepend(partialEl);

function clearPartial(): void {
  partialEl.textContent = "";
  partialEl.classList.add("hidden");
}

void listen<{ text: string }>("partial", (e) => {
  if (e.payload.text.length === 0) {
    clearPartial();
    return;
  }
  partialEl.textContent = e.payload.text;
  partialEl.classList.remove("hidden");
  // Only re-pin when a finalized phrase actually landed above it — re-prepending
  // an already-first node restarts its CSS phrase-in animation every tick.
  if (phrases.firstElementChild !== partialEl) {
    phrases.prepend(partialEl);
  }
});

// ── Phrase listener ───────────────────────────────────────────────────────────

void listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  (e) => {
    clearPartial();
    const li = document.createElement("li");
    if (e.payload.error) {
      li.textContent = e.payload.error;
      li.classList.add("error");
    } else {
      const pair = selectedLangPair();
      const tgt = document.createElement("span");
      tgt.style.color = "var(--text)";
      tgt.textContent = `${pair.tgt.toUpperCase()}: ${e.payload.translated_text}`;

      // Canary AST produces no source-language transcript (source_text === "").
      // Show only the translated text instead of an empty source line with an
      // arrow pointing at nothing.
      if (e.payload.source_text) {
        const src = document.createElement("span");
        src.style.color = "var(--muted)";
        src.textContent = `${pair.src.toUpperCase()}: ${e.payload.source_text}`;

        const arrow = document.createElement("span");
        arrow.style.color = "var(--accent)";
        arrow.textContent = " → ";

        li.append(src, arrow, tgt);
      } else {
        li.append(tgt);
      }
    }
    phrases.prepend(li);
  },
);

// ── Engine warmup listeners ───────────────────────────────────────────────────
// The engine binds its port immediately and loads models in the background;
// Rust polls /health and relays real progress. That progress arrives in coarse
// jumps (one step per model: ~33 → 66 → 100), which looks janky shown raw. We
// animate a displayed value that eases toward each real milestone and creeps a
// bounded amount between them, so the bar feels alive without ever faking
// completion — it never reaches 100% until the engine reports it.

let warmupTarget = 0; // latest real progress from the backend
let warmupDisplay = 0; // animated value actually shown
let warmupTimer: ReturnType<typeof setInterval> | null = null;

const warmupBox = document.querySelector<HTMLDivElement>("#warmup-progress")!;
const warmupFill = document.querySelector<HTMLDivElement>("#warmup-bar-fill")!;

function stopWarmupAnim(): void {
  if (warmupTimer !== null) {
    clearInterval(warmupTimer);
    warmupTimer = null;
  }
  warmupBox.classList.add("hidden");
}

function startWarmupAnim(): void {
  warmupBox.classList.remove("hidden");
  if (warmupTimer !== null) return;
  warmupTimer = setInterval(() => {
    // Yield the status line if something else took it over (session started,
    // error, etc.) so we never fight another writer.
    if (!statusEl.textContent?.startsWith("Loading translation models")) {
      stopWarmupAnim();
      return;
    }
    // Creep toward a soft ceiling just above the real target; hold there until
    // the next milestone. Only a real 100 unlocks the last stretch.
    const ceiling = warmupTarget >= 100 ? 100 : Math.min(95, warmupTarget + 15);
    if (warmupDisplay < ceiling) {
      warmupDisplay = Math.min(ceiling, warmupDisplay + Math.max(0.4, (ceiling - warmupDisplay) * 0.05));
    }
    statusEl.textContent = `Loading translation models… ${Math.round(warmupDisplay)}%`;
    warmupFill.style.width = `${Math.round(warmupDisplay)}%`;
    if (warmupDisplay >= 100) stopWarmupAnim();
  }, 80);
}

void listen("engine-starting", () => {
  warmupTarget = 0;
  warmupDisplay = 0;
  statusEl.textContent = "Loading translation models… 0%";
  warmupFill.style.width = "0%";
  startWarmupAnim();
});

void listen<{ progress: number }>("engine-warmup-progress", (e) => {
  warmupTarget = Math.max(warmupTarget, e.payload.progress);
  startWarmupAnim();
});

// ── PTT state listener ────────────────────────────────────────────────────────
// Only show the recording indicator when in PTT mode — VAD mode ignores it.

void listen<boolean>("ptt-state", (e) => {
  recIndicator.classList.toggle("hidden", mode !== "ptt" || !e.payload);
});

// PTT diagnostics — log `ptt-diag` events to the browser console so the user
// can see them when running in dev mode (F12 → Console).
void listen<any>("ptt-diag", (e) => {
  console.log("[ptt-diag]", JSON.stringify(e.payload));
});

// ── Mode selector ─────────────────────────────────────────────────────────────

modeSelector.querySelectorAll<HTMLButtonElement>(".mode-btn").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const newMode = btn.dataset["mode"] as "vad" | "ptt";

    // Block mode switch only when actively translating in VAD
    if (mode === "vad" && listening) return;

    // If leaving PTT mid-session, stop first
    if (mode === "ptt" && listening) {
      await stopSession();
    }

    // Hide recording indicator when leaving PTT mode
    if (mode === "ptt" && newMode !== "ptt") {
      recIndicator.classList.add("hidden");
    }

    mode = newMode;
    modeSelector
      .querySelectorAll(".mode-btn")
      .forEach((b) => b.classList.remove("active"));
    btn.classList.add("active");
    pttConfig.classList.toggle("hidden", mode !== "ptt");
    toggle.classList.toggle("hidden", mode === "ptt");

    // If entering PTT with a shortcut, auto-start
    if (mode === "ptt" && pttShortcut && !listening) {
      await startSession();
    } else if (mode === "ptt" && !pttShortcut) {
      statusEl.textContent = "Set a PTT shortcut first";
    }
  });
});

// ── Shortcut capture ──────────────────────────────────────────────────────────

captureBtn.addEventListener("click", () => {
  isCapturingShortcut = true;
  shortcutDisplay.textContent = "Press a key…";
  shortcutDisplay.classList.add("capturing");
  captureBtn.disabled = true;
});

window.addEventListener("keydown", async (e) => {
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
    try {
      await invoke("register_ptt_shortcut", { shortcutStr });
      pttShortcut = shortcutStr;
      localStorage.setItem("lt.pttShortcut", shortcutStr);
      shortcutDisplay.textContent = shortcutStr;
      shortcutDisplay.classList.remove("capturing");
      isCapturingShortcut = false;
      captureBtn.disabled = false;
      captureBtn.setAttribute("data-shortcut-registered", "true");

      // Auto-start if already in PTT mode
      if (mode === "ptt" && !listening) {
        await startSession();
      }
    } catch (err) {
      shortcutDisplay.textContent = `Error: ${err}`;
      shortcutDisplay.classList.remove("capturing");
      isCapturingShortcut = false;
      captureBtn.disabled = false;
      setTimeout(() => {
        if (shortcutDisplay.textContent?.startsWith("Error:")) {
          shortcutDisplay.textContent = "Not set";
        }
      }, 4000);
    }
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
  // Default to the virtual audio cable (VB-Cable on Windows, BlackHole on
  // macOS) so the translated voice reaches the call app without the user
  // hunting for it — unless they previously picked a different output.
  outputSelect.value = pickDefaultOutputDevice(outputs, localStorage.getItem("lt.outputDevice"));
}

outputSelect.addEventListener("change", () => {
  localStorage.setItem("lt.outputDevice", outputSelect.value);
});

// ── Toggle UI helper ──────────────────────────────────────────────────────────

function setToggle(active: boolean): void {
  const icon = toggle.querySelector<HTMLSpanElement>(".toggle-icon")!;
  const label = toggle.querySelector<HTMLSpanElement>(".toggle-label")!;
  // Disable mode buttons only in VAD mode; PTT mode buttons stay enabled for switching
  modeSelector
    .querySelectorAll<HTMLButtonElement>(".mode-btn")
    .forEach((b) => (b.disabled = active && mode === "vad"));

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

// ── Shared session helpers ─────────────────────────────────────────────────────

async function startSession(): Promise<void> {
  toggle.disabled = true;
  try {
    const cmd =
      mode === "ptt" ? "start_live_translation_ptt" : "start_live_translation";
    const pair = selectedLangPair();
    await invoke(cmd, {
      deviceName: inputSelect.value,
      outputDeviceName: outputSelect.value,
      useClonedVoice: cloneEnabled(),
      sourceLang: pair.src,
      targetLang: pair.tgt,
    });
    listening = true;
    setToggle(true);
    // The pair is captured by the session at start — lock the selector so the
    // UI can't drift from what the engine is actually doing.
    langPairSelect.disabled = true;
    if (overlayEnabled()) void (await overlay())?.show();
  } catch (err) {
    statusEl.textContent = `Error: ${err}`;
  } finally {
    toggle.disabled = false;
  }
}

async function stopSession(): Promise<void> {
  toggle.disabled = true;
  try {
    await invoke("stop_live_translation");
    listening = false;
    setToggle(false);
    langPairSelect.disabled = false;
    // A stale dimmed partial line must never survive a stop — the Rust worker
    // may have a partial decode in flight when stop lands.
    clearPartial();
    void (await overlay())?.hide();
  } catch (err) {
    statusEl.textContent = `Error: ${err}`;
  } finally {
    toggle.disabled = false;
  }
}

// ── Start / stop ──────────────────────────────────────────────────────────────

toggle.addEventListener("click", async () => {
  if (!listening) {
    await startSession();
  } else {
    await stopSession();
  }
});

// ── Restore persisted PTT shortcut ─────────────────────────────────────────────
// The shortcut is registered in the Rust global-shortcut layer, which does not
// survive an app restart. Re-register the saved combo and restore the display so
// the user doesn't have to set it again every launch.

async function restorePttShortcut(): Promise<void> {
  const saved = localStorage.getItem("lt.pttShortcut");
  if (!saved) return;
  try {
    await invoke("register_ptt_shortcut", { shortcutStr: saved });
    pttShortcut = saved;
    shortcutDisplay.textContent = saved;
    captureBtn.setAttribute("data-shortcut-registered", "true");
  } catch (err) {
    // The combo may now be taken by another app — drop it so the UI is honest.
    console.error("restore PTT shortcut failed:", err);
    localStorage.removeItem("lt.pttShortcut");
  }
}

// ── Window close cleanup ───────────────────────────────────────────────────────

window.addEventListener("beforeunload", () => {
  if (listening) {
    stopSession();
  }
});

void loadDevices();
void restorePttShortcut();
