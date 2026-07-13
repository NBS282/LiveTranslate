import { describe, expect, it } from "vitest";
import { appendLogLine } from "./setup-log";

describe("appendLogLine", () => {
  it("appends the first detail line to an empty log", () => {
    expect(appendLogLine("", "Downloading Piper voice model")).toBe(
      "Downloading Piper voice model",
    );
  });

  it("appends a new line when the previous line was not a progress update", () => {
    const log = appendLogLine("Downloading Piper voice model", "1.0 / 705.3 MB");
    expect(log).toBe("Downloading Piper voice model\n1.0 / 705.3 MB");
  });

  it("replaces the last line when both it and the new detail are progress updates", () => {
    let log = appendLogLine("Downloading native STT model", "1.0 / 705.3 MB");
    log = appendLogLine(log, "2.5 / 705.3 MB");
    expect(log).toBe("Downloading native STT model\n2.5 / 705.3 MB");
  });

  it("appends a new line when the new detail is not a progress update, even after progress lines", () => {
    let log = appendLogLine("Downloading native STT model", "1.0 / 705.3 MB");
    log = appendLogLine(log, "Setup complete");
    expect(log).toBe("Downloading native STT model\n1.0 / 705.3 MB\nSetup complete");
  });

  it("ignores an empty detail", () => {
    expect(appendLogLine("Downloading Piper voice model", "")).toBe(
      "Downloading Piper voice model",
    );
  });
});
