import { describe, expect, it } from "vitest";
import { clampPercent, isClickable, statusLabel, type UpdateState } from "./update";

describe("clampPercent", () => {
  it("returns 0 when the total is unknown", () => {
    expect(clampPercent(1234, 0)).toBe(0);
    expect(clampPercent(1234, -1)).toBe(0);
  });

  it("computes a rounded percent of the total", () => {
    expect(clampPercent(0, 200)).toBe(0);
    expect(clampPercent(50, 200)).toBe(25);
    expect(clampPercent(1, 3)).toBe(33);
  });

  it("clamps to 100 when downloaded meets or exceeds the total", () => {
    expect(clampPercent(200, 200)).toBe(100);
    expect(clampPercent(250, 200)).toBe(100);
  });

  it("never returns a negative percent", () => {
    expect(clampPercent(-10, 200)).toBe(0);
  });
});

describe("statusLabel", () => {
  it("labels each state in Spanish", () => {
    expect(statusLabel({ kind: "idle" })).toBe("Buscar actualizaciones");
    expect(statusLabel({ kind: "checking" })).toBe("Buscando…");
    expect(statusLabel({ kind: "available", version: "0.6.0" })).toBe("Actualización disponible");
    expect(statusLabel({ kind: "downloading", percent: 42 })).toBe("Descargando 42%…");
    expect(statusLabel({ kind: "installing" })).toBe("Instalando…");
    expect(statusLabel({ kind: "up-to-date" })).toBe("Estás al día");
    expect(statusLabel({ kind: "error" })).toBe("Error al actualizar");
  });
});

describe("isClickable", () => {
  it("is clickable only when idle or an update is available", () => {
    const clickable: UpdateState[] = [{ kind: "idle" }, { kind: "available", version: "0.6.0" }];
    for (const state of clickable) expect(isClickable(state)).toBe(true);
  });

  it("is not clickable while checking, downloading, or installing", () => {
    const blocked: UpdateState[] = [
      { kind: "checking" },
      { kind: "downloading", percent: 10 },
      { kind: "installing" },
      { kind: "up-to-date" },
      { kind: "error" },
    ];
    for (const state of blocked) expect(isClickable(state)).toBe(false);
  });
});
