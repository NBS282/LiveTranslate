# LiveTranslate UI/UX Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the violet/cyan gradient UI with a monochrome "Handy" aesthetic, simplify first-run setup to hide technical detail, rebuild onboarding as a 4-step explanatory wizard, and add a persisted on/off toggle for the subtitle overlay.

**Architecture:** Pure frontend change. `styles.css` owns a centralized token system; `index.html` holds static structure; `main.ts` owns screen routing + friendly-text mapping + wizard logic; `live.ts` owns the overlay toggle's live effect; `overlay.html` is self-contained and repainted. No Rust backend changes.

**Tech Stack:** TypeScript (DOM, no framework), Vite, Tauri 2 (`@tauri-apps/api`), plain CSS.

## Global Constraints

- Package manager: `pnpm` only. Never `npm`/`yarn`.
- No automated test infra exists (no vitest/jest). Validation per task = `pnpm build` passes (tsc typecheck) + a concrete manual check via `pnpm tauri dev`.
- All UI copy and code identifiers in artifacts: English by default, EXCEPT user-facing Spanish copy explicitly specified in the spec (this app's onboarding/setup text is Spanish per the user's request — copy the Spanish strings verbatim from each task).
- Palette tokens (exact hex): `--bg #0A0A0B`, `--surface #141416`, `--surface-2 #1B1B1E`, `--border #232327`, `--text #EDEDEF`, `--muted #6B6B70`, `--accent #6366F1`, `--accent-dim #4F46E5`, `--danger #E5484D`.
- No `linear-gradient` backgrounds, no `backdrop-filter` glassmorphism anywhere.
- Accent color only on active state (Start button idle, active mode segment, current wizard step, overlay toggle ON, focus rings).

---

### Task 1: Visual system foundation (monochrome tokens)

Reskins the entire app at once by swapping the token palette and removing every gradient/glassmorphism usage. After this task all three screens already look monochrome; later tasks restructure content.

**Files:**
- Modify: `src/styles.css` (`:root` block lines 3-17; plus every `--violet`/`--violet-dim`/`--cyan` reference and the gradient/glassmorphism blocks)

**Interfaces:**
- Produces: CSS custom properties `--bg --surface --surface-2 --border --text --muted --accent --accent-dim --danger` available to all files. No `--violet`/`--cyan` tokens remain.

- [ ] **Step 1: Replace the `:root` token block**

In `src/styles.css`, replace lines 3-17 (`:root { ... }`) with:

```css
:root {
  --bg:          #0A0A0B;
  --surface:     #141416;
  --surface-2:   #1B1B1E;
  --border:      #232327;
  --accent:      #6366F1;
  --accent-dim:  #4F46E5;
  --danger:      #E5484D;
  --text:        #EDEDEF;
  --muted:       #6B6B70;
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  font-size: 15px;
  line-height: 1.55;
}
```

- [ ] **Step 2: Replace all old accent references**

In `src/styles.css`, do these exact replacements (replace-all):
- `var(--violet-dim)` → `var(--accent-dim)`
- `var(--violet)` → `var(--accent)`
- `var(--cyan)` → `var(--accent)`
- `var(--green)` → `var(--accent)` (status "ok" dots become accent, not green)
- `var(--red)` → `var(--danger)`

- [ ] **Step 3: Remove the logo-mark gradient**

Replace the `.logo-mark` block (was lines 53-66) background line. The block becomes:

```css
.logo-mark {
  width: 44px;
  height: 44px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 11px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  font-size: 0.95rem;
  color: var(--text);
  letter-spacing: -0.5px;
  flex-shrink: 0;
}
```

- [ ] **Step 4: Remove the progress-fill gradient**

Replace the `.progress-fill` `background` line so the block reads:

```css
.progress-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 999px;
  transition: width 0.3s ease;
  min-width: 4px;
}
```

- [ ] **Step 5: Remove onboarding glassmorphism**

Replace the `#screen-onboarding .card` block (was lines 279-288) with a flat surface:

```css
#screen-onboarding .card {
  background: var(--surface);
  border-color: var(--border);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.45);
}
```

- [ ] **Step 6: Tune headings tracking**

Confirm `h1` and `h2` blocks include `letter-spacing: -0.3px` and `-0.2px` respectively (already present — leave as is). No change needed; this step is a verification checkpoint.

- [ ] **Step 7: Typecheck**

Run: `pnpm build`
Expected: PASS (CSS is not typechecked, but tsc + vite build must complete with no errors).

- [ ] **Step 8: Manual visual check**

Run: `pnpm tauri dev`. Expected: the app window renders in neutral black/gray; the `LT` logo is a flat dark square (no purple→cyan gradient); the Start button and active mode segment use indigo `#6366F1`; no purple glow anywhere.

- [ ] **Step 9: Commit**

```bash
git add src/styles.css
git commit -m "style: monochrome Handy palette, remove gradients and glassmorphism"
```

---

### Task 2: Simplified setup screen

Removes the technical checklist and raw pip log from view; shows a friendly status line + bar + percentage, with a collapsible "Ver detalles" for troubleshooting.

**Files:**
- Modify: `index.html` (`#screen-setup`, was lines 13-43)
- Modify: `src/main.ts` (setup-progress listener lines 50-61; setup-done listener lines 99-112; add a friendly-text mapper)
- Modify: `src/styles.css` (add `.details-toggle` style)

**Interfaces:**
- Consumes: Rust `setup-progress` event `{ step: string; percent: number; detail: string }` and `setup-done` event `{ success: boolean; error?: string }` (unchanged).
- Produces: `friendlySetupText(step: string): string` in `main.ts`.

- [ ] **Step 1: Rewrite the setup screen markup**

In `index.html`, replace the entire `#screen-setup` block (lines 13-43) with:

```html
    <!-- ── SETUP ─────────────────────────────────────────────────────── -->
    <div id="screen-setup" class="screen hidden">
      <div class="card">
        <div class="logo-mark">LT</div>
        <h1>LiveTranslate</h1>
        <p class="muted">Preparando tu instalación. Esto se hace una sola vez.</p>

        <button id="btn-run-setup" class="btn-primary btn-full">Comenzar instalación</button>

        <div id="setup-progress-box" class="progress-section hidden">
          <div class="progress-header">
            <span id="setup-step">Preparando todo para ti…</span>
            <span id="setup-pct">0%</span>
          </div>
          <div class="progress-track">
            <div id="setup-bar-fill" class="progress-fill" style="width:0%"></div>
          </div>
          <button id="setup-details-toggle" class="details-toggle" type="button">Ver detalles</button>
          <pre id="setup-log" class="hidden"></pre>
        </div>
      </div>
    </div>
```

- [ ] **Step 2: Add the friendly-text mapper to main.ts**

In `src/main.ts`, add this function near the top (after the imports, before `show`):

```ts
function friendlySetupText(step: string): string {
  const s = step.toLowerCase();
  if (s.includes("python") || s.includes("environment") || s.includes("pip") || s.includes("extract"))
    return "Preparando todo para ti…";
  if (s.includes("pytorch") || s.includes("engine") || s.includes("packages"))
    return "Instalando el motor de traducción…";
  if (s.includes("voice") || s.includes("piper") || s.includes("model"))
    return "Descargando la voz…";
  if (s.includes("complete"))
    return "Casi listo…";
  return "Preparando todo para ti…";
}
```

- [ ] **Step 3: Use the mapper in the setup-progress listener**

In `src/main.ts`, replace the `setup-progress` listener body (lines 54-61) with:

```ts
listen<{ step: string; percent: number; detail: string }>("setup-progress", (e) => {
  const { step, percent, detail } = e.payload;
  setProgress("setup-bar-fill", "setup-pct", "setup-step", percent, friendlySetupText(step));
  if (detail) {
    setupLog.textContent += `${detail}\n`;
    setupLog.scrollTop = setupLog.scrollHeight;
  }
});
```

- [ ] **Step 4: Wire the "Ver detalles" disclosure + friendly failure**

In `src/main.ts`, replace the `btnRunSetup` click handler and `setup-done` listener (lines 89-112) with:

```ts
const btnRunSetup = document.getElementById("btn-run-setup") as HTMLButtonElement;

btnRunSetup.addEventListener("click", () => {
  btnRunSetup.disabled = true;
  btnRunSetup.textContent = "Instalando…";
  document.getElementById("setup-progress-box")!.classList.remove("hidden");
  setupLog.textContent = "";
  void invoke("start_setup");
});

document.getElementById("setup-details-toggle")!.addEventListener("click", () => {
  const log = document.getElementById("setup-log")!;
  const hidden = log.classList.toggle("hidden");
  document.getElementById("setup-details-toggle")!.textContent = hidden ? "Ver detalles" : "Ocultar detalles";
});

listen<{ success: boolean; error?: string }>("setup-done", (e) => {
  if (e.payload.success) {
    show("screen-onboarding");
    setStepDot(1);
    void checkVBCable();
  } else {
    document.getElementById("setup-step")!.textContent = "Algo salió mal. Mirá los detalles e intentá de nuevo.";
    document.getElementById("setup-log")!.classList.remove("hidden");
    document.getElementById("setup-details-toggle")!.textContent = "Ocultar detalles";
    btnRunSetup.disabled = false;
    btnRunSetup.textContent = "Reintentar";
  }
});
```

Note: the success branch no longer calls `setCheckIcon` (the checklist is gone) and no longer calls `checkPiper` (voice download moves out of onboarding — Task 3). `setCheckIcon` becomes unused; remove its definition (lines 12-15) in this step.

- [ ] **Step 5: Update init() — remove checklist references**

In `src/main.ts`, replace the `init()` body (lines 65-85) with:

```ts
async function init(): Promise<void> {
  try {
    const status = await invoke<{ venv_ok: boolean; piper_voice_ok: boolean; ready: boolean }>(
      "check_setup",
    );
    if (status.ready) {
      show("screen-onboarding");
      setStepDot(1);
      void checkVBCable();
    } else {
      show("screen-setup");
    }
  } catch (err) {
    console.error("init failed:", err);
    show("screen-setup");
  }
}
```

- [ ] **Step 6: Add the details-toggle style**

In `src/styles.css`, add after the `#setup-log` block:

```css
.details-toggle {
  background: transparent;
  border: none;
  color: var(--muted);
  font-size: 0.75rem;
  padding: 0.25rem 0;
  align-self: flex-start;
  text-decoration: underline;
  text-underline-offset: 3px;
}
.details-toggle:hover { color: var(--text); }
```

- [ ] **Step 7: Typecheck**

Run: `pnpm build`
Expected: PASS. (If tsc reports `setCheckIcon` declared but never read, confirm you removed its definition in Step 4.)

- [ ] **Step 8: Manual check**

Run: `pnpm tauri dev`. Force the setup screen by temporarily renaming `%LocalAppData%\com.livetranslate.app\engine` (or open DevTools console and run `document.querySelectorAll('.screen').forEach(s=>s.classList.add('hidden')); document.getElementById('screen-setup').classList.remove('hidden')`). Expected: no checklist items, no terminal log visible; only friendly text + bar + %; "Ver detalles" reveals/hides the log.

- [ ] **Step 9: Commit**

```bash
git add index.html src/main.ts src/styles.css
git commit -m "feat(setup): friendly progress, hide technical detail behind disclosure"
```

---

### Task 3: 4-step onboarding wizard

Rebuilds onboarding as VB-Cable → connect audio → shortcuts → done. Removes the voice-model download step (Setup already downloads it). Updates step indicator to 4 steps.

**Files:**
- Modify: `index.html` (`#screen-onboarding`, was lines 46-117)
- Modify: `src/main.ts` (step navigation; `setStepDot` signature; reopen handler; remove `checkPiper` + piper download handlers)
- Modify: `src/styles.css` (add `.audio-diagram` style)

**Interfaces:**
- Consumes: Rust commands `check_vbcable` (returns `boolean`) and `check_setup` (unchanged).
- Produces: `setStepDot(active: 1 | 2 | 3 | 4): void` (widened from `1 | 2 | 3`).

- [ ] **Step 1: Rewrite the onboarding markup**

In `index.html`, replace the entire `#screen-onboarding` block (lines 46-117) with:

```html
    <!-- ── ONBOARDING ────────────────────────────────────────────────── -->
    <div id="screen-onboarding" class="screen hidden">
      <div class="card">
        <div class="step-dots">
          <div class="step-dot active"></div>
          <div class="step-line"></div>
          <div class="step-dot"></div>
          <div class="step-line"></div>
          <div class="step-dot"></div>
          <div class="step-line"></div>
          <div class="step-dot"></div>
        </div>

        <!-- Step 1: VB-Cable -->
        <div id="ob-step-1" class="ob-step">
          <div>
            <h2>Cable de audio virtual</h2>
            <p class="muted">Necesitás VB-Cable para que Zoom o Discord escuchen el audio traducido como si fuera un micrófono.</p>
          </div>
          <div id="ob-cable-status" class="status-badge">
            <span class="badge-dot"></span>
            <span class="badge-text">Verificando…</span>
          </div>
          <div class="btn-row">
            <button id="btn-ob-cable-check" class="btn-secondary">Re-verificar</button>
            <button id="ob-cable-install" class="btn-link hidden">Descargar VB-Cable →</button>
            <button id="btn-ob-next-1" class="btn-primary hidden">Continuar →</button>
          </div>
        </div>

        <!-- Step 2: Connect the audio -->
        <div id="ob-step-2" class="ob-step hidden">
          <div>
            <h2>Conectá el audio</h2>
            <p class="muted">Así viaja tu voz traducida hasta la videollamada:</p>
          </div>
          <div class="audio-diagram">
            <div class="audio-row"><span>Tu micrófono</span><span class="arrow">→</span><span>LiveTranslate</span></div>
            <div class="audio-row"><span>LiveTranslate</span><span class="arrow">→</span><span><em>CABLE Input</em> (elegilo como Output acá)</span></div>
            <div class="audio-row"><span><em>CABLE Output</em></span><span class="arrow">→</span><span>micrófono en Discord / Zoom / Teams</span></div>
          </div>
          <button id="btn-ob-next-2" class="btn-primary btn-full">Continuar →</button>
        </div>

        <!-- Step 3: Shortcuts -->
        <div id="ob-step-3" class="ob-step hidden">
          <div>
            <h2>Atajos de teclado</h2>
            <p class="muted">Para el modo Push-to-talk podés definir un atajo:</p>
          </div>
          <ul class="instructions">
            <li>Es un <strong>combo de 2 o más teclas</strong> (por ejemplo <em>Ctrl + Shift + Espacio</em>).</li>
            <li>Lo apretás para <strong>empezar y frenar</strong> la grabación.</li>
            <li>Funciona mientras <strong>LiveTranslate esté abierto</strong>, aunque la ventana no esté en foco.</li>
          </ul>
          <button id="btn-ob-next-3" class="btn-primary btn-full">Continuar →</button>
        </div>

        <!-- Step 4: Done -->
        <div id="ob-step-4" class="ob-step hidden">
          <div>
            <h2>¡Todo listo!</h2>
            <p class="muted">Ya podés empezar a traducir en tus llamadas.</p>
          </div>
          <button id="btn-ob-finish" class="btn-primary btn-full">Empezar a usar LiveTranslate →</button>
        </div>
      </div>
    </div>
```

- [ ] **Step 2: Widen setStepDot**

In `src/main.ts`, replace the `setStepDot` signature (line 41) so it reads:

```ts
function setStepDot(active: 1 | 2 | 3 | 4): void {
```

(The body already iterates over all `.step-dot` elements, so it works for 4 dots unchanged.)

- [ ] **Step 3: Replace step-1 navigation**

In `src/main.ts`, replace the `btn-ob-next-1` handler (lines 147-152) with:

```ts
document.getElementById("btn-ob-next-1")!.addEventListener("click", () => {
  document.getElementById("ob-step-1")!.classList.add("hidden");
  document.getElementById("ob-step-2")!.classList.remove("hidden");
  setStepDot(2);
});
```

- [ ] **Step 4: Remove the piper voice step logic**

In `src/main.ts`, delete the entire "Onboarding step 2: Piper voice" section (was lines 154-208): the `checkPiper` function, the `btnDlPiper` const + click handler, and the old `btn-ob-next-2` handler. Replace that whole region with the new step-2 and step-3 navigation:

```ts
// ── Onboarding step 2: connect audio ─────────────────────────────────────────

document.getElementById("btn-ob-next-2")!.addEventListener("click", () => {
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.remove("hidden");
  setStepDot(3);
});

// ── Onboarding step 3: shortcuts ─────────────────────────────────────────────

document.getElementById("btn-ob-next-3")!.addEventListener("click", () => {
  document.getElementById("ob-step-3")!.classList.add("hidden");
  document.getElementById("ob-step-4")!.classList.remove("hidden");
  setStepDot(4);
});
```

- [ ] **Step 5: Keep the finish handler**

Confirm the `btn-ob-finish` handler (was lines 212-215) still reads:

```ts
document.getElementById("btn-ob-finish")!.addEventListener("click", () => {
  show("screen-main");
  void import("./live.ts");
});
```

No change needed; verification checkpoint.

- [ ] **Step 6: Fix the reopen-onboarding handler**

In `src/main.ts`, replace the `btn-reopen-onboarding` handler (lines 219-240) with:

```ts
document.getElementById("btn-reopen-onboarding")!.addEventListener("click", () => {
  show("screen-onboarding");
  setStepDot(1);

  document.getElementById("ob-step-1")!.classList.remove("hidden");
  document.getElementById("ob-step-2")!.classList.add("hidden");
  document.getElementById("ob-step-3")!.classList.add("hidden");
  document.getElementById("ob-step-4")!.classList.add("hidden");

  document.getElementById("btn-ob-next-1")!.classList.add("hidden");
  document.getElementById("ob-cable-install")!.classList.add("hidden");

  setStatusBadge("ob-cable-status", "idle", "Verificando…");
  void checkVBCable();
});
```

- [ ] **Step 7: Update the VB-Cable badge copy**

In `src/main.ts`, in `checkVBCable` (lines 118-139), update the user-facing strings: `"Checking…"`→`"Verificando…"`, `"VB-Cable detected"`→`"VB-Cable detectado"`, `"Not found — install VB-Cable then click Re-check"`→`"No encontrado — instalá VB-Cable y volvé a verificar"`, and the button text `"Checking…"`→`"Verificando…"` / `"Re-check"`→`"Re-verificar"`.

- [ ] **Step 8: Add the audio-diagram style**

In `src/styles.css`, add after the `.instructions` block:

```css
.audio-diagram {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.audio-row {
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  gap: 0.6rem;
  padding: 0.6rem 0.75rem;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 9px;
  font-size: 0.8rem;
  color: var(--muted);
}
.audio-row em { color: var(--text); font-style: normal; font-weight: 500; }
.audio-row .arrow { color: var(--accent); }
.audio-row span:last-child { text-align: right; }
```

- [ ] **Step 9: Typecheck**

Run: `pnpm build`
Expected: PASS with no "declared but never read" errors (confirm `checkPiper`/`btnDlPiper` fully removed in Step 4).

- [ ] **Step 10: Manual check**

Run: `pnpm tauri dev`. From the onboarding screen, walk all 4 steps. Expected: step indicator has 4 dots; step 2 shows the 3-row audio diagram; step 3 explains the shortcut combo; step 4 finishes into the main screen. The `?` (gear) button on main reopens onboarding at step 1.

- [ ] **Step 11: Commit**

```bash
git add index.html src/main.ts src/styles.css
git commit -m "feat(onboarding): 4-step explanatory wizard, drop redundant voice download"
```

---

### Task 4: Overlay enable/disable toggle

Adds a persisted on/off switch on the main screen controlling the subtitle overlay.

**Files:**
- Modify: `index.html` (`#screen-main`, add a toggle field after the mode selector)
- Modify: `src/live.ts` (read preference before `overlay().show()`; add `setOverlayEnabled`; wire the switch)
- Modify: `src/styles.css` (add `.switch` styles)

**Interfaces:**
- Consumes: `WebviewWindow.getByLabel("overlay")` via the existing async `overlay()` helper in `live.ts`.
- Produces: `localStorage` key `lt.overlayEnabled` (`"1"`/`"0"`); helper `overlayEnabled(): boolean`.

- [ ] **Step 1: Add the toggle markup**

In `index.html`, inside `#screen-main`, immediately after the `Translation mode` field block (the `<div class="field">` containing `#mode-selector`, ends ~line 148), insert:

```html
        <div class="field switch-field">
          <label class="field-label" for="overlay-toggle">Subtítulos en pantalla</label>
          <button id="overlay-toggle" class="switch" role="switch" aria-checked="true" type="button">
            <span class="switch-knob"></span>
          </button>
        </div>
```

- [ ] **Step 2: Add the switch styles**

In `src/styles.css`, add after the `.mode-selector`/`.mode-btn` blocks:

```css
.switch-field {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
}
.switch {
  width: 42px;
  height: 24px;
  border-radius: 999px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  padding: 2px;
  display: flex;
  align-items: center;
  transition: background 0.18s, border-color 0.18s;
}
.switch .switch-knob {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--muted);
  transition: transform 0.18s, background 0.18s;
}
.switch[aria-checked="true"] {
  background: var(--accent);
  border-color: var(--accent);
}
.switch[aria-checked="true"] .switch-knob {
  transform: translateX(18px);
  background: #fff;
}
```

- [ ] **Step 3: Add the preference helper and wiring in live.ts**

In `src/live.ts`, after the `overlay()` helper (lines 24-26), add:

```ts
// ── Overlay enable/disable preference ─────────────────────────────────────────

const overlayToggle = document.querySelector<HTMLButtonElement>("#overlay-toggle")!;

function overlayEnabled(): boolean {
  return localStorage.getItem("lt.overlayEnabled") !== "0";
}

function setOverlayChecked(on: boolean): void {
  overlayToggle.setAttribute("aria-checked", on ? "true" : "false");
}

// Reflect the stored preference on load.
setOverlayChecked(overlayEnabled());

overlayToggle.addEventListener("click", async () => {
  const next = !overlayEnabled();
  localStorage.setItem("lt.overlayEnabled", next ? "1" : "0");
  setOverlayChecked(next);
  // Apply live: hide immediately if turned off; show if a session is active.
  if (next) {
    if (listening) void (await overlay())?.show();
  } else {
    void (await overlay())?.hide();
  }
});
```

- [ ] **Step 4: Respect the preference on start/stop**

In `src/live.ts`, in the toggle click handler (lines 155-183), change the start branch line `void (await overlay())?.show();` to:

```ts
      if (overlayEnabled()) void (await overlay())?.show();
```

(The stop branch `void (await overlay())?.hide();` stays unchanged.)

- [ ] **Step 5: Typecheck**

Run: `pnpm build`
Expected: PASS.

- [ ] **Step 6: Manual check**

Run: `pnpm tauri dev`. On the main screen: the switch defaults ON (indigo). Start translation with it ON → overlay appears. Toggle OFF mid-session → overlay hides immediately. Toggle ON → overlay reappears. Stop, set OFF, Start again → overlay never appears. Reload the app → the switch remembers its last state.

- [ ] **Step 7: Commit**

```bash
git add index.html src/live.ts src/styles.css
git commit -m "feat(overlay): persisted on/off toggle for on-screen subtitles"
```

---

### Task 5: Repaint the overlay to the neutral palette

The overlay window has its own inline `<style>`; bring it in line with the monochrome system (remove the violet border + glow, change the cursor accent).

**Files:**
- Modify: `overlay.html` (inline `<style>`, lines 39-87)

**Interfaces:**
- Consumes: nothing (self-contained window).
- Produces: nothing.

- [ ] **Step 1: Repaint the backdrop**

In `overlay.html`, replace the `.backdrop` block (lines 39-51) with:

```css
      .backdrop {
        position: absolute;
        inset: 0;
        background: rgba(10, 10, 11, 0.82);
        backdrop-filter: blur(20px) saturate(1.2);
        -webkit-backdrop-filter: blur(20px) saturate(1.2);
        border-radius: 12px;
        border: 1px solid rgba(35, 35, 39, 0.9);
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.55);
      }
```

(Note: the overlay keeps `backdrop-filter` because it must be legible over arbitrary screen content — the global "no glassmorphism" rule targets the app cards, not the floating subtitle. The violet border + violet glow are removed.)

- [ ] **Step 2: Recolor source/cursor**

In `overlay.html`, in `#src-text` (lines 60-67) change `color: rgba(100, 116, 139, 0.85);` to `color: rgba(107, 107, 112, 0.9);`. In `.cursor` (lines 78-87) change `background: #22d3ee;` to `background: #6366F1;`.

- [ ] **Step 3: Typecheck**

Run: `pnpm build`
Expected: PASS (overlay.ts unaffected; build bundles overlay.html).

- [ ] **Step 4: Manual check**

Run: `pnpm tauri dev`, start a translation (overlay ON). Expected: the floating subtitle has a neutral dark backdrop with a thin gray border (no purple edge/glow); the typed cursor is indigo.

- [ ] **Step 5: Commit**

```bash
git add overlay.html
git commit -m "style(overlay): neutral palette, drop violet border and glow"
```

---

## Self-Review

**Spec coverage:**
- §1 Visual system → Task 1 ✓
- §2 Setup screen (hide checklist + log, friendly text, Ver detalles, friendly failure) → Task 2 ✓
- §3 Onboarding 4-step wizard + reopen, voice download moved out → Task 3 ✓
- §4 Overlay toggle (localStorage, live effect, default ON) → Task 4; overlay repaint → Task 5 ✓
- Component boundaries (styles/index/main/live/overlay) → respected across tasks ✓
- Error handling (setup failure, VB-Cable not found, defensive localStorage read) → Task 2 Step 4, Task 3 Step 7, Task 4 Step 3 (`!== "0"` defaults ON on missing/invalid) ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✓

**Type consistency:** `setStepDot` widened to `1|2|3|4` in Task 3 Step 2 and only ever called with 1-4. `overlayEnabled()`/`setOverlayChecked()` defined once (Task 4) and reused. `friendlySetupText` defined Task 2 Step 2, used Step 3. `checkPiper`/`btnDlPiper`/`setCheckIcon` removals are paired with their last usages. ✓

**Note for executor:** Line numbers reference the pre-edit files; after each task they shift. Match on the quoted code, not the line number.
