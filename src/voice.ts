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

// Optimistic hint, persisted across launches: did we ever save a cloned voice?
// Lets us show "preparing" immediately on open instead of a confusing "generic"
// flash while the engine is still loading. The server is the source of truth and
// reconciles this flag once it responds.
function hasStoredProfile(): boolean {
  return localStorage.getItem("lt.hasClonedVoice") === "1";
}

function setStoredProfile(on: boolean): void {
  localStorage.setItem("lt.hasClonedVoice", on ? "1" : "0");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── Status ────────────────────────────────────────────────────────────────────

function setPreparing(): void {
  statusLabel.textContent = "Preparando voz clonada…";
  cloneToggle.disabled = true;
  deleteBtn.classList.add("hidden");
}

export async function loadVoiceStatus(): Promise<void> {
  // The Python server answers no request until model warmup finishes, so a
  // successful status response means the cloned voice is genuinely ready to use.
  // While the engine loads, keep the toggle disabled: show "preparing" if we
  // know a voice was saved before, otherwise leave the default generic state.
  //
  // Recording is also blocked until the engine is up: the record flow ends in
  // an upload that needs the server listening, and on first launch warmup can
  // take several minutes (models may still be downloading).
  recordBtn.disabled = true;
  recordBtn.title = "El motor de traducción se está preparando…";
  if (hasStoredProfile()) {
    setPreparing();
  } else {
    statusLabel.textContent = "Preparando motor…";
  }

  // Poll until the engine responds — warmup can take minutes on first launch,
  // longer if the first-run model download is still in progress.
  // 450 attempts * 2s ≈ 15 min, comfortably beyond a slow first setup.
  for (let attempt = 0; attempt < 450; attempt++) {
    try {
      const exists = await invoke<boolean>("get_voice_profile_status");
      updateUI(exists);
      setStoredProfile(exists);
      return;
    } catch {
      await sleep(2000);
    }
  }

  // Engine never came up. Leave the stored flag untouched so a later reload can
  // still detect the profile; keep recording blocked and say so honestly.
  statusLabel.textContent = "Motor no disponible — reiniciá la aplicación";
}

function updateUI(profileExists: boolean): void {
  // Reaching this point means the engine answered — recording is safe now.
  recordBtn.disabled = false;
  recordBtn.title = "";
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
    setStoredProfile(false);
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
  clearTimerInterval();
  recStatusEl.textContent = "Procesando voz… (tarda unos segundos)";

  await new Promise<void>((resolve) => {
    mediaRecorder!.onstop = () => resolve();
    mediaRecorder!.stop();
    mediaRecorder!.stream.getTracks().forEach((t) => t.stop());
  });

  const blob = new Blob(recordedChunks, { type: "audio/webm" });
  const arrayBuffer = await blob.arrayBuffer();
  const audioCtx = new AudioContext();
  const audioBuffer = await audioCtx.decodeAudioData(arrayBuffer);
  void audioCtx.close();
  const wavBytes = encodeWavPcm16Mono(audioBuffer);
  const audioData = Array.from(wavBytes);

  try {
    await invoke("upload_voice_profile", { audioData });
    recStatusEl.textContent = "¡Voz guardada!";
    updateUI(true);
    setStoredProfile(true);
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

// ── WAV encoder ──────────────────────────────────────────────────────────────

function encodeWavPcm16Mono(audioBuffer: AudioBuffer): Uint8Array {
  const numCh = audioBuffer.numberOfChannels;
  const len = audioBuffer.length;
  const sampleRate = audioBuffer.sampleRate;

  // Downmix all channels to mono by averaging samples.
  const mono = new Float32Array(len);
  for (let ch = 0; ch < numCh; ch++) {
    const data = audioBuffer.getChannelData(ch);
    for (let i = 0; i < len; i++) {
      mono[i] += data[i] / numCh;
    }
  }

  const bytesPerSample = 2; // 16-bit PCM
  const blockAlign = bytesPerSample; // mono: 1 channel * 2 bytes
  const byteRate = sampleRate * blockAlign;
  const dataSize = len * bytesPerSample;
  const buffer = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buffer);
  let off = 0;

  const writeStr = (s: string): void => {
    for (let i = 0; i < s.length; i++) {
      view.setUint8(off++, s.charCodeAt(i));
    }
  };

  // RIFF chunk
  writeStr("RIFF");
  view.setUint32(off, 36 + dataSize, true); off += 4;
  writeStr("WAVE");

  // fmt sub-chunk
  writeStr("fmt ");
  view.setUint32(off, 16, true); off += 4;      // sub-chunk size
  view.setUint16(off, 1, true); off += 2;       // PCM format
  view.setUint16(off, 1, true); off += 2;       // mono
  view.setUint32(off, sampleRate, true); off += 4;
  view.setUint32(off, byteRate, true); off += 4;
  view.setUint16(off, blockAlign, true); off += 2;
  view.setUint16(off, 16, true); off += 2;      // bits per sample

  // data sub-chunk
  writeStr("data");
  view.setUint32(off, dataSize, true); off += 4;

  // PCM samples: float → 16-bit signed little-endian
  for (let i = 0; i < len; i++) {
    const samp: number = mono[i] ?? 0;
    const s = Math.max(-1, Math.min(1, samp));
    view.setInt16(off, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    off += 2;
  }

  return new Uint8Array(buffer);
}

// ── Init ──────────────────────────────────────────────────────────────────────

void loadVoiceStatus();
