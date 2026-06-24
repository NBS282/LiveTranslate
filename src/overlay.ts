import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { primaryMonitor, LogicalPosition } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const subtitle = document.getElementById("subtitle")!;
const srcText = document.getElementById("src-text")!;
const tgtChars = document.getElementById("tgt-chars")!;

const win = getCurrentWebviewWindow();

let hideTimer: ReturnType<typeof setTimeout> | null = null;
let streamTimer: ReturnType<typeof setTimeout> | null = null;
let showEnabled = true;

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

  // Split into tokens (each word plus its trailing whitespace) so spacing is
  // preserved and we can highlight one word at a time.
  const tokens = text.match(/\S+\s*/g) ?? [];

  // Pre-create a span per word; we reveal them char by char below.
  const wordSpans = tokens.map(() => {
    const span = document.createElement("span");
    span.className = "word";
    tgtChars.appendChild(span);
    return span;
  });

  // Blinking cursor that rides at the typing edge.
  const cursor = document.createElement("span");
  cursor.className = "cursor";

  let w = 0; // current word index
  let c = 0; // char index within the current token
  function tick(): void {
    if (w >= tokens.length) {
      // Done — settle everything to the resting (white) color, drop the cursor.
      wordSpans.forEach((s) => s.classList.remove("active"));
      cursor.remove();
      return;
    }
    const token = tokens[w]!;
    const span = wordSpans[w]!;
    // The word currently being typed glows yellow; finished words are white.
    span.classList.add("active");
    span.textContent = token.slice(0, c + 1);
    span.after(cursor); // keep the cursor right after the active word
    c++;
    if (c >= token.length) {
      span.classList.remove("active");
      w++;
      c = 0;
    }
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

// The main window tells us whether on-screen subtitles are enabled.
void listen<boolean>("overlay-toggle", (e) => {
  showEnabled = e.payload;
  if (!showEnabled) {
    if (hideTimer !== null) clearTimeout(hideTimer);
    cancelStream();
    subtitle.classList.remove("visible");
    void win.hide();
  }
});

void listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  async (e) => {
    if (e.payload.error) return;
    if (!showEnabled) return;

    await win.show();
    subtitle.classList.add("visible");
    srcText.textContent = `ES: ${e.payload.source_text}`;
    streamText(e.payload.translated_text);
    scheduleHide();
  },
);

void init();
