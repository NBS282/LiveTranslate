import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inputSelect = document.querySelector<HTMLSelectElement>("#live-device")!;
const outputSelect =
  document.querySelector<HTMLSelectElement>("#live-output-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#live-status")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
let listening = false;

async function loadDevices() {
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

listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  (e) => {
    const li = document.createElement("li");
    if (e.payload.error) {
      li.textContent = `⚠ ${e.payload.error}`;
    } else {
      li.textContent = `ES: ${e.payload.source_text}  →  EN: ${e.payload.translated_text}`;
    }
    phrases.appendChild(li);
  }
);

toggle.addEventListener("click", async () => {
  try {
    if (!listening) {
      await invoke("start_live_translation", {
        deviceName: inputSelect.value,
        outputDeviceName: outputSelect.value,
      });
      listening = true;
      toggle.textContent = "Stop";
      status.textContent = "Listening…";
    } else {
      await invoke("stop_live_translation");
      listening = false;
      toggle.textContent = "Listen";
      status.textContent = "Idle";
    }
  } catch (err) {
    status.textContent = `Error: ${err}`;
  }
});

loadDevices();
