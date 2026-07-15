// Picks which output device to pre-select in the "Output (sends to the call)"
// dropdown. The translated audio must land on the virtual audio cable so the
// call app can pick it up as a microphone, so we default to that cable:
// VB-Cable's "CABLE Input" on Windows, "BlackHole" on macOS. Only the relevant
// one exists on each platform, so matching either name is enough. A device the
// user previously chose wins over the default. Kept free of DOM/Tauri imports
// so it can be unit-tested (see output-device.test.ts).

const VIRTUAL_CABLE_RE = /cable input|blackhole/i;

export function pickDefaultOutputDevice(devices: string[], saved: string | null): string {
  if (devices.length === 0) return "";
  if (saved && devices.includes(saved)) return saved;
  const cable = devices.find((name) => VIRTUAL_CABLE_RE.test(name));
  return cable ?? devices[0]!;
}
