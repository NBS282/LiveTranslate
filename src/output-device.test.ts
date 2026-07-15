import { describe, expect, it } from "vitest";
import { pickDefaultOutputDevice } from "./output-device";

describe("pickDefaultOutputDevice", () => {
  it("returns an empty string when there are no devices", () => {
    expect(pickDefaultOutputDevice([], null)).toBe("");
  });

  it("defaults to VB-Cable on Windows", () => {
    const devices = ["Speakers (Realtek)", "CABLE Input (VB-Audio Virtual Cable)", "Headphones"];
    expect(pickDefaultOutputDevice(devices, null)).toBe("CABLE Input (VB-Audio Virtual Cable)");
  });

  it("defaults to BlackHole on macOS", () => {
    const devices = ["MacBook Pro Speakers", "BlackHole 2ch"];
    expect(pickDefaultOutputDevice(devices, null)).toBe("BlackHole 2ch");
  });

  it("prefers a previously chosen device that still exists", () => {
    const devices = ["Speakers", "CABLE Input (VB-Audio Virtual Cable)", "Headphones"];
    expect(pickDefaultOutputDevice(devices, "Headphones")).toBe("Headphones");
  });

  it("falls back to the cable when the saved device is gone", () => {
    const devices = ["Speakers", "CABLE Input (VB-Audio Virtual Cable)"];
    expect(pickDefaultOutputDevice(devices, "Unplugged USB")).toBe(
      "CABLE Input (VB-Audio Virtual Cable)",
    );
  });

  it("falls back to the first device when no cable is present", () => {
    const devices = ["Speakers (Realtek)", "Headphones"];
    expect(pickDefaultOutputDevice(devices, null)).toBe("Speakers (Realtek)");
  });
});
