import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { primaryMonitor, LogicalPosition } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const subtitle = document.getElementById("subtitle")!;
const srcText = document.getElementById("src-text")!;
const tgtChars = document.getElementById("tgt-chars")!;

const win = getCurrentWebviewWindow();

let hideTimer: ReturnType<typeof setTimeout> | null = null;
let streamTimer: ReturnType<typeof setTimeout> | null = null;

async function init(): Promise<void> {
  await win.setIgnoreCursorEvents(true);

  const monitor = await primaryMonitor();
  if (monitor) {
    const scale = monitor.scaleFactor;
    const screenW = monitor.size.width / scale;
    const screenH = monitor.size.height / scale;
    const winW = 720;
    const winH = 130;
    await win.setPosition(
      new LogicalPosition(
        Math.round((screenW - winW) / 2),
        Math.round(screenH - winH - 76),
      ),
    );
  }
}

function cancelStream(): void {
  if (streamTimer !== null) {
    clearTimeout(streamTimer);
    streamTimer = null;
  }
}

function streamText(text: string): void {
  cancelStream();
  tgtChars.textContent = "";

  const cursor = document.createElement("span");
  cursor.className = "cursor";
  tgtChars.appendChild(cursor);

  let i = 0;
  function tick(): void {
    if (i >= text.length) {
      cursor.remove();
      return;
    }
    // Insert the next character before the cursor
    cursor.insertAdjacentText("beforebegin", text[i]!);
    i++;
    streamTimer = setTimeout(tick, 26);
  }
  tick();
}

function scheduleHide(): void {
  if (hideTimer !== null) clearTimeout(hideTimer);
  hideTimer = setTimeout(() => {
    subtitle.classList.remove("visible");
    setTimeout(() => void win.hide(), 280);
  }, 9000);
}

void listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  async (e) => {
    if (e.payload.error) return;

    await win.show();
    subtitle.classList.add("visible");
    srcText.textContent = `ES: ${e.payload.source_text}`;
    streamText(e.payload.translated_text);
    scheduleHide();
  },
);

void init();
