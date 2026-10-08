import type { Terminal } from "@xterm/xterm";

export type TerminalFitMode = "shell" | "tui";

const DEFAULT_SCROLLBAR_WIDTH = 14;
/**
 * The alt screen has no scrollback to scroll and its scrollbar is hidden, so
 * the full pane width belongs to the grid.
 */
const TUI_GUTTER_WIDTH = 0;

type CellSize = { width: number; height: number };

export function terminalScrollbarWidth(
  overviewRuler?: { width?: number },
): number {
  const width = overviewRuler?.width;
  return width === undefined ? DEFAULT_SCROLLBAR_WIDTH : width;
}

function cellSize(term: Terminal): CellSize | null {
  const dims = (
    term as unknown as {
      _core?: {
        _renderService?: { dimensions?: { css?: { cell?: CellSize } } };
      };
    }
  )._core?._renderService?.dimensions?.css?.cell;
  if (!dims || dims.width < 1 || dims.height < 1) return null;
  return dims;
}

function availableSize(
  host: HTMLElement,
  mode: TerminalFitMode,
  term: Terminal,
): { width: number; height: number } | null {
  const gutter =
    mode === "tui"
      ? TUI_GUTTER_WIDTH
      : terminalScrollbarWidth(term.options.overviewRuler);
  const width = host.clientWidth - gutter;
  const height = host.clientHeight;
  if (width < 8 || height < 8) return null;
  return { width, height };
}

export function fitTerminal(
  term: Terminal,
  host: HTMLElement,
  mode: TerminalFitMode,
): { cols: number; rows: number } | null {
  const size = availableSize(host, mode, term);
  if (!size) return null;

  const cell = cellSize(term);
  if (!cell) return null;

  // Round down, in both modes. A grid rounded up, or stretched to cover the
  // pane, reaches past the host, which clips it — and what a full-screen TUI
  // loses that way is its bottom line. The strip left over is under one cell;
  // the terminal paints no background of its own, so nothing shows there.
  const cols = Math.max(2, Math.floor(size.width / cell.width));
  const rows = Math.max(1, Math.floor(size.height / cell.height));

  if (term.cols !== cols || term.rows !== rows) {
    term.resize(cols, rows);
  }

  return { cols: term.cols, rows: term.rows };
}

export function applyTerminalChrome(
  term: Terminal,
  outer: HTMLElement,
  tui: boolean,
): void {
  outer.classList.toggle("monocode-terminal--alt-screen", tui);
  // xterm's decoration overview ruler paints a 1px outline down the whole
  // right edge, which reads as a border the TUI did not draw. The alt screen
  // has no scrollback to summarise, so leave the ruler off.
  term.options.overviewRuler = {};
}
