import { invoke } from "@tauri-apps/api/core";

const statusLabel = document.querySelector<HTMLSpanElement>("#voice-status-label")!;
const recordBtn = document.querySelector<HTMLButtonElement>("#btn-record-voice")!;
const deleteBtn = document.querySelector<HTMLButtonElement>("#btn-delete-voice")!;
const cloneToggle = document.querySelector<HTMLButtonElement>("#voice-clone-toggle")!;

const modal = document.querySelector<HTMLDivElement>("#voice-record-modal")!;
const startRecBtn = document.querySelector<HTMLButtonElement>("#btn-start-recording")!;
const stopRecBtn = document.querySelector<HTMLButtonElement>("#btn-stop-recording")!;
const cancelRecBtn = document.querySelector<HTMLButtonElement>("#btn-cancel-recording")!;
const timerEl = document.querySelector<HTMLDivElement>("#voice-record-timer")!;
const recStatusEl = document.querySelector<HTMLParagraphElement>("#voice-record-status")!;

let mediaRecorder: MediaRecorder | null = null;
let recordedChunks: Blob[] = [];
let timerInterval: ReturnType<typeof setInterval> | null = null;
let elapsedSeconds = 0;

// ── Persistence helpers ───────────────────────────────────────────────────────

export function cloneEnabled(): boolean {
  return localStorage.getItem("lt.useClonedVoice") === "1";
}

function setCloneEnabled(on: boolean): void {
  localStorage.setItem("lt.useClonedVoice", on ? "1" : "0");
  cloneToggle.setAttribute("aria-checked", on ? "true" : "false");
}

// ── Status ────────────────────────────────────────────────────────────────────

export async function loadVoiceStatus(): Promise<void> {
  try {
    const exists = await invoke<boolean>("get_voice_profile_status");
    updateUI(exists);
  } catch {
    updateUI(false);
  }
}

function updateUI(profileExists: boolean): void {
  if (profileExists) {
    statusLabel.textContent = "Voz clonada lista";
    deleteBtn.classList.remove("hidden");
    cloneToggle.disabled = false;
  } else {
    statusLabel.textContent = "Genérica (Piper)";
    deleteBtn.classList.add("hidden");
    cloneToggle.disabled = true;
    setCloneEnabled(false);
  }
  const checked = profileExists && cloneEnabled();
  cloneToggle.setAttribute("aria-checked", checked ? "true" : "false");
}

// ── Toggle ────────────────────────────────────────────────────────────────────

cloneToggle.addEventListener("click", () => {
  if (cloneToggle.disabled) return;
  setCloneEnabled(!cloneEnabled());
});

// ── Delete profile ────────────────────────────────────────────────────────────

deleteBtn.addEventListener("click", async () => {
  try {
    await invoke("delete_voice_profile");
    updateUI(false);
  } catch (err) {
    console.error("delete voice profile failed:", err);
  }
});

// ── Recording ─────────────────────────────────────────────────────────────────

recordBtn.addEventListener("click", () => {
  modal.classList.remove("hidden");
  recStatusEl.textContent = "";
  timerEl.textContent = "0s";
});

cancelRecBtn.addEventListener("click", () => {
  stopRecording();
  modal.classList.add("hidden");
});

startRecBtn.addEventListener("click", async () => {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    recordedChunks = [];
    mediaRecorder = new MediaRecorder(stream);
    mediaRecorder.ondataavailable = (e) => {
      if (e.data.size > 0) recordedChunks.push(e.data);
    };
    mediaRecorder.start(100);

    elapsedSeconds = 0;
    timerEl.textContent = "0s";
    timerInterval = setInterval(() => {
      elapsedSeconds++;
      timerEl.textContent = `${elapsedSeconds}s`;
    }, 1000);

    startRecBtn.classList.add("hidden");
    stopRecBtn.classList.remove("hidden");
    recStatusEl.textContent = "Grabando… hablá con naturalidad.";
  } catch (err) {
    recStatusEl.textContent = `Error de micrófono: ${err}`;
  }
});

stopRecBtn.addEventListener("click", async () => {
  if (!mediaRecorder) return;
  stopRecBtn.disabled = true;
  recStatusEl.textContent = "Procesando voz… (tarda unos segundos)";

  await new Promise<void>((resolve) => {
    mediaRecorder!.onstop = () => resolve();
    mediaRecorder!.stop();
    mediaRecorder!.stream.getTracks().forEach((t) => t.stop());
  });

  clearTimerInterval();

  const blob = new Blob(recordedChunks, { type: "audio/webm" });
  const arrayBuffer = await blob.arrayBuffer();
  const audioData = Array.from(new Uint8Array(arrayBuffer));

  try {
    await invoke("upload_voice_profile", { audioData });
    recStatusEl.textContent = "¡Voz guardada!";
    updateUI(true);
    setTimeout(() => modal.classList.add("hidden"), 1200);
  } catch (err) {
    recStatusEl.textContent = `Error: ${err}`;
  } finally {
    stopRecBtn.disabled = false;
    startRecBtn.classList.remove("hidden");
    stopRecBtn.classList.add("hidden");
    mediaRecorder = null;
  }
});

function stopRecording(): void {
  if (mediaRecorder && mediaRecorder.state !== "inactive") {
    mediaRecorder.stop();
    mediaRecorder.stream.getTracks().forEach((t) => t.stop());
    mediaRecorder = null;
  }
  clearTimerInterval();
  startRecBtn.classList.remove("hidden");
  stopRecBtn.classList.add("hidden");
  stopRecBtn.disabled = false;
}

function clearTimerInterval(): void {
  if (timerInterval !== null) {
    clearInterval(timerInterval);
    timerInterval = null;
  }
}

// ── Init ──────────────────────────────────────────────────────────────────────

void loadVoiceStatus();
