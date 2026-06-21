import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inputSelect = document.querySelector<HTMLSelectElement>("#live-device")!;
const outputSelect = document.querySelector<HTMLSelectElement>("#live-output-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#live-status")!;
const statusDot = document.querySelector<HTMLSpanElement>("#status-dot")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
let listening = false;

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

listen<{ source_text: string; translated_text: string; error: string | null }>(
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
      arrow.style.color = "var(--violet)";
      arrow.textContent = " → ";

      const tgt = document.createElement("span");
      tgt.style.color = "var(--cyan)";
      tgt.textContent = `EN: ${e.payload.translated_text}`;

      li.append(src, arrow, tgt);
    }
    phrases.prepend(li);
  },
);

function setToggle(active: boolean): void {
  const icon = toggle.querySelector<HTMLSpanElement>(".toggle-icon")!;
  const label = toggle.querySelector<HTMLSpanElement>(".toggle-label")!;
  if (active) {
    icon.textContent = "■";
    label.textContent = "Stop";
    toggle.classList.add("running");
    statusDot.classList.add("active");
    statusEl.textContent = "Listening…";
  } else {
    icon.textContent = "▶";
    label.textContent = "Start";
    toggle.classList.remove("running");
    statusDot.classList.remove("active");
    statusEl.textContent = "Idle";
  }
}

toggle.addEventListener("click", async () => {
  toggle.disabled = true;
  try {
    if (!listening) {
      await invoke("start_live_translation", {
        deviceName: inputSelect.value,
        outputDeviceName: outputSelect.value,
      });
      listening = true;
      setToggle(true);
    } else {
      await invoke("stop_live_translation");
      listening = false;
      setToggle(false);
    }
  } catch (err) {
    statusEl.textContent = `Error: ${err}`;
  } finally {
    toggle.disabled = false;
  }
});

void loadDevices();
