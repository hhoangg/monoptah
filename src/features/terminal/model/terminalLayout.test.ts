import { describe, expect, it } from "vitest";
import type { Terminal } from "@xterm/xterm";
import {
  applyTerminalChrome,
  fitTerminal,
  terminalScrollbarWidth,
} from "./terminalLayout";

describe("terminalScrollbarWidth", () => {
  it("defaults to 14px when overview ruler width is unset", () => {
    expect(terminalScrollbarWidth(undefined)).toBe(14);
    expect(terminalScrollbarWidth({})).toBe(14);
  });

  it("honors an explicit overview ruler width", () => {
    expect(terminalScrollbarWidth({ width: 1 })).toBe(1);
    expect(terminalScrollbarWidth({ width: 0 })).toBe(0);
  });
});

/**
 * Stands in for xterm. The cell it reports follows `letterSpacing` and
 * `lineHeight` the way xterm's own measurement does — quantised to whole
 * device pixels — so a fit that tries to stretch the grid over the pane shows
 * up here as a grid that overshoots it.
 */
function fakeTerm(base: { width: number; height: number }, dpr = 2) {
  const options = {
    letterSpacing: 0,
    lineHeight: 1,
    overviewRuler: {} as { width?: number },
  };
  const cell = {
    get width() {
      return (base.width * dpr + Math.round(options.letterSpacing)) / dpr;
    },
    get height() {
      return Math.floor(base.height * dpr * options.lineHeight) / dpr;
    },
  };
  const term = {
    cols: 80,
    rows: 24,
    options,
    resize(cols: number, rows: number) {
      term.cols = cols;
      term.rows = rows;
    },
    _core: { _renderService: { dimensions: { css: { cell } } } },
    cell,
  };
  return term;
}

function host(width: number, height: number) {
  return { clientWidth: width, clientHeight: height } as HTMLElement;
}

describe("fitTerminal", () => {
  it("keeps the TUI grid inside the host so the bottom row stays visible", () => {
    const term = fakeTerm({ width: 8, height: 17 });
    const pane = host(1005, 600);

    const size = fitTerminal(term as unknown as Terminal, pane, "tui");

    expect(size).not.toBeNull();
    expect(term.cols * term.cell.width).toBeLessThanOrEqual(1005);
    expect(term.rows * term.cell.height).toBeLessThanOrEqual(600);
  });

  it("leaves under one cell of the TUI pane uncovered", () => {
    const term = fakeTerm({ width: 8, height: 17 });
    const pane = host(1005, 600);

    fitTerminal(term as unknown as Terminal, pane, "tui");

    expect(1005 - term.cols * term.cell.width).toBeLessThan(term.cell.width);
    expect(600 - term.rows * term.cell.height).toBeLessThan(term.cell.height);
  });

  it("gives the TUI the full pane width, with no scrollbar gutter", () => {
    const term = fakeTerm({ width: 8, height: 17 });

    const size = fitTerminal(term as unknown as Terminal, host(1000, 600), "tui");

    expect(size).toEqual({ cols: 125, rows: 35 });
  });

  it("settles on the same TUI grid when fitted again at the same size", () => {
    const term = fakeTerm({ width: 8, height: 17 });
    const pane = host(1005, 600);

    const first = fitTerminal(term as unknown as Terminal, pane, "tui");
    const second = fitTerminal(term as unknown as Terminal, pane, "tui");

    expect(second).toEqual(first);
  });

  it("keeps the shell grid unstretched and clear of the scrollbar", () => {
    const term = fakeTerm({ width: 8, height: 17 });

    const size = fitTerminal(
      term as unknown as Terminal,
      host(1005, 600),
      "shell",
    );

    expect(size).toEqual({ cols: Math.floor((1005 - 14) / 8), rows: 35 });
    expect(term.options.letterSpacing).toBe(0);
    expect(term.options.lineHeight).toBe(1);
  });
});

describe("applyTerminalChrome", () => {
  it("never enables the overview ruler, which outlines the right edge", () => {
    const term = fakeTerm({ width: 8, height: 17 });
    const outer = { classList: { toggle: () => {} } } as unknown as HTMLElement;

    applyTerminalChrome(term as unknown as Terminal, outer, true);

    expect(term.options.overviewRuler.width).toBeUndefined();
  });
});
