import { invoke } from "@tauri-apps/api/core";

const select = document.querySelector<HTMLSelectElement>("#device")!;
const toggle = document.querySelector<HTMLButtonElement>("#toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#status")!;
let running = false;

async function loadDevices() {
  const devices = await invoke<string[]>("get_output_devices");
  select.replaceChildren();
  for (const name of devices) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = name;
    select.appendChild(opt);
  }
}

toggle.addEventListener("click", async () => {
  if (!running) {
    await invoke("start_passthrough", { outputName: select.value });
    running = true;
    toggle.textContent = "Stop";
    status.textContent = `Running -> ${select.value}`;
  } else {
    await invoke("stop_passthrough");
    running = false;
    toggle.textContent = "Start";
    status.textContent = "Stopped";
  }
});

loadDevices();
