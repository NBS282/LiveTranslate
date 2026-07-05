import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { primaryMonitor, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

const subtitle = document.getElementById("subtitle")!;
const srcText = document.getElementById("src-text")!;
const tgtChars = document.getElementById("tgt-chars")!;

const win = getCurrentWebviewWindow();

let hideTimer: ReturnType<typeof setTimeout> | null = null;
let fadeTimer: ReturnType<typeof setTimeout> | null = null;
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

/// Resize the window to fit the subtitle card snugly, then re-center
/// at the bottom of the screen.
const MIN_WIN_W = 320;
const WIN_PAD = 12;

async function resizeToContent(): Promise<void> {
  await new Promise((r) => requestAnimationFrame(r));

  const card = document.querySelector<HTMLElement>(".subtitle-card");
  if (!card) return;
  const { width, height } = card.getBoundingClientRect();

  const w = Math.max(MIN_WIN_W, Math.ceil(width + WIN_PAD * 2));
  const h = Math.ceil(height + WIN_PAD * 2);

  await win.setSize(new LogicalSize(w, h));

  // Re-center at bottom of screen
  const monitor = await primaryMonitor();
  if (monitor) {
    const scale = monitor.scaleFactor;
    const screenW = monitor.size.width / scale;
    const screenH = monitor.size.height / scale;
    await win.setPosition(
      new LogicalPosition(
        Math.round((screenW - w) / 2),
        Math.round(screenH - h - 40),
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

/// Cancel the whole hide chain — both the 9s delay and the in-flight 280ms
/// fade-out step, so a caption shown inside that window is not hidden under it.
function cancelHide(): void {
  if (hideTimer !== null) {
    clearTimeout(hideTimer);
    hideTimer = null;
  }
  if (fadeTimer !== null) {
    clearTimeout(fadeTimer);
    fadeTimer = null;
  }
}

function scheduleHide(): void {
  cancelHide();
  hideTimer = setTimeout(() => {
    hideTimer = null;
    subtitle.classList.remove("visible");
    fadeTimer = setTimeout(() => {
      fadeTimer = null;
      void win.hide();
    }, 280);
  }, 9000);
}

// The main window tells us whether on-screen subtitles are enabled.
void listen<boolean>("overlay-toggle", (e) => {
  showEnabled = e.payload;
  if (!showEnabled) {
    epoch++; // abort in-flight partial/phrase continuations
    cancelHide();
    cancelStream();
    clearPartial();
    subtitle.classList.remove("visible");
    void win.hide();
  }
});

// ── Partial (in-progress) translation ─────────────────────────────────────────
// While a segment is open the engine streams a rough preview (`partial` event,
// ~1.2s cadence). It reuses the caption card's target line, dimmed and italic,
// and is cleared when the final phrase lands or an empty partial marks the
// segment closed.

let partialActive = false;

// Monotonic generation counter. `partial` and `phrase` are independent async
// listeners fed by unsynchronized Rust threads, and each suspends at awaits
// (win.show, resizeToContent) — so an in-flight continuation can resume AFTER
// a newer event already rewrote the caption. Every event that takes ownership
// of the caption (a phrase, a segment close, an overlay disable) bumps the
// epoch; older continuations compare their captured value after each await
// and bail out instead of clobbering newer DOM state.
let epoch = 0;

function clearPartial(): void {
  partialActive = false;
  tgtChars.classList.remove("subtitle-partial");
}

void listen<{ text: string }>("partial", async (e) => {
  if (!showEnabled) return;

  if (e.payload.text.length === 0) {
    // Segment closed — drop the preview; the final phrase (if any) re-shows.
    epoch++; // invalidate in-flight partial continuations
    if (!partialActive) return;
    clearPartial();
    tgtChars.textContent = "";
    subtitle.classList.remove("visible");
    scheduleHide();
    return;
  }

  const myEpoch = epoch; // stale-check token for the awaits below

  cancelStream();
  if (!partialActive) {
    partialActive = true;
    tgtChars.classList.add("subtitle-partial");
    srcText.textContent = ""; // partials carry no source text
  }
  cancelHide(); // keep the card up while the segment is live

  await win.show();
  if (myEpoch !== epoch) return; // a phrase/close/disable won the race
  subtitle.classList.add("visible");
  tgtChars.textContent = e.payload.text;
  await resizeToContent();
  // No DOM writes after the last await — nothing left to guard.
});

void listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  async (e) => {
    // The finalized phrase is the authority over the caption: bump the epoch so
    // in-flight partial continuations go stale, and capture it so a NEWER
    // phrase can in turn invalidate this handler at its own awaits.
    const myEpoch = ++epoch;

    if (e.payload.error) {
      // An errored segment still closes the live preview.
      if (partialActive) {
        clearPartial();
        tgtChars.textContent = "";
        subtitle.classList.remove("visible");
        scheduleHide();
      }
      return;
    }
    clearPartial();
    if (!showEnabled) return;

    cancelHide(); // a pending fade must not hide the window we're about to show
    await win.show();
    if (myEpoch !== epoch) return; // a newer phrase took over
    subtitle.classList.add("visible");
    // Canary AST produces no Spanish transcript (source_text === ""). Hide the
    // "ES:" line entirely instead of showing an empty label.
    if (e.payload.source_text) {
      srcText.textContent = `ES: ${e.payload.source_text}`;
      srcText.style.display = "";
    } else {
      srcText.textContent = "";
      srcText.style.display = "none";
    }
    // Fill the full text so the card has its final size when we measure.
    tgtChars.textContent = e.payload.translated_text;
    await resizeToContent();
    if (myEpoch !== epoch) return; // a newer phrase took over
    // Re-assert the final presentation: a same-epoch partial (the next
    // segment's preview racing our awaits — partials read the epoch but never
    // bump it) may have re-added the dimmed style between our continuations.
    // clearPartial touches only the class and flag — never #src-text, which we
    // just filled — and this tail runs synchronously with the check above, so
    // a partial arriving later still takes the slot legitimately.
    clearPartial();
    // Now clear and start the char-by-char streaming effect.
    streamText(e.payload.translated_text);
    scheduleHide();
  },
);

void init();
