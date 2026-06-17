import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const select = document.querySelector<HTMLSelectElement>("#live-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#live-status")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
let listening = false;

async function loadInputDevices() {
  // reuse the output-device command? we need input devices; if not present, the default mic is used.
  // For now, leave the dropdown optional: empty value = default device.
  const opt = document.createElement("option");
  opt.value = ""; opt.textContent = "Default microphone";
  select.replaceChildren(opt);
}

listen<{ source_text: string; translated_text: string; error: string | null }>("phrase", (e) => {
  const li = document.createElement("li");
  if (e.payload.error) {
    li.textContent = `⚠ ${e.payload.error}`;
  } else {
    li.textContent = `ES: ${e.payload.source_text}  →  EN: ${e.payload.translated_text}`;
  }
  phrases.appendChild(li);
});

toggle.addEventListener("click", async () => {
  try {
    if (!listening) {
      await invoke("start_live_translation", { deviceName: select.value });
      listening = true; toggle.textContent = "Stop"; status.textContent = "Listening…";
    } else {
      await invoke("stop_live_translation");
      listening = false; toggle.textContent = "Listen"; status.textContent = "Idle";
    }
  } catch (err) {
    status.textContent = `Error: ${err}`;
  }
});

loadInputDevices();
