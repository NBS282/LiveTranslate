import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

const pick = document.querySelector<HTMLButtonElement>("#pick")!;
const chosen = document.querySelector<HTMLSpanElement>("#chosen")!;
const translate = document.querySelector<HTMLButtonElement>("#translate")!;
const status = document.querySelector<HTMLParagraphElement>("#tr-status")!;
const player = document.querySelector<HTMLAudioElement>("#player")!;
const text = document.querySelector<HTMLPreElement>("#tr-text")!;

let inputPath: string | null = null;

pick.addEventListener("click", async () => {
  const selected = await open({
    multiple: false,
    filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg", "m4a"] }],
  });
  if (typeof selected === "string") {
    inputPath = selected;
    chosen.textContent = selected;
    translate.disabled = false;
  }
});

translate.addEventListener("click", async () => {
  if (!inputPath) return;
  translate.disabled = true;
  status.textContent = "Translating… (CPU; first run loads models, can take a bit)";
  try {
    const res = await invoke<{ output_wav: string; source_text: string; translated_text: string }>(
      "translate_file",
      { inputPath },
    );
    player.src = convertFileSrc(res.output_wav);
    text.textContent = `ES: ${res.source_text}\nEN: ${res.translated_text}`;
    status.textContent = "Done.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  } finally {
    translate.disabled = false;
  }
});
