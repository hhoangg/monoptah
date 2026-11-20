// The one place that touches xterm.js. xterm and its stylesheet are bundled from npm and loaded here by dynamic import, so
// they are a separate chunk that only a terminal tab or drawer pulls in (never a CDN, never a third-party script).
//
// What this file deliberately does NOT do (security.md "Terminal"):
//  - no link handling: hyperlinks the program prints (OSC 8) activate nothing, and there is no web links addon;
//  - no clipboard addon: the program cannot write to the clipboard (OSC 52); copy and paste are user actions;
//  - no title: `onTitleChange` is never subscribed, so a program cannot rename the tab or put text into the page;
//  - no innerHTML: xterm draws with DOM nodes and text, and the output is only ever given to `Terminal.write`.
import type { Terminal } from "@xterm/xterm";
import type { TerminalScreen } from "./port";

/** The screen the controller draws on, plus what the page needs around it. */
export interface XtermScreen extends TerminalScreen {
  focus(): void;
  hasSelection(): boolean;
  getSelection(): string;
  /** Pastes as the user would (bracketed paste when the program asked for it); the data comes back through `onData`. */
  paste(text: string): void;
  /** Whether arrow keys must be sent as application sequences (`ESC O A`). */
  applicationCursor(): boolean;
  fit(): void;
  dispose(): void;
}

export interface ScreenHandlers {
  onData(data: string): void;
  onResize(cols: number, rows: number): void;
}

const FONT = 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace';
/** A dark terminal in both color schemes: the default palette of the 16 colors is built for a dark background. */
const THEME = { background: "#131313", foreground: "#ebebeb", cursor: "#4c9ef5", selectionBackground: "#4c9ef566" } as const;

const isMac = (): boolean => /Mac|iPhone|iPad/.test(navigator.platform);

export async function createXtermScreen(host: HTMLElement, handlers: ScreenHandlers): Promise<XtermScreen> {
  const [{ Terminal: XTerm }, { FitAddon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit"), import("@xterm/xterm/css/xterm.css")]);
  const term: Terminal = new XTerm({
    cursorBlink: true,
    scrollback: 5000,
    fontFamily: FONT,
    fontSize: 14,
    theme: THEME,
    allowProposedApi: false,
    macOptionIsMeta: true,
    // A link the program prints is text, not a way out: activation does nothing (no navigation, no confirm).
    linkHandler: { activate: () => undefined },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);

  // Ctrl/Cmd+C copies while text is selected (else it is ^C); Cmd+V, Ctrl+V and Ctrl+Shift+C/V use the browser: the
  // `copy` and `paste` events xterm listens to do the work. Returning false hands the key to the browser.
  term.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    const key = event.key.toLowerCase();
    const mod = isMac() ? event.metaKey : event.ctrlKey;
    if (!mod || event.altKey) return true;
    if (key === "v") return false;
    if (key === "c") return !(event.shiftKey || isMac() || term.hasSelection());
    return true;
  });

  const subscriptions = [term.onData((data) => handlers.onData(data)), term.onResize(({ cols, rows }) => handlers.onResize(cols, rows))];
  let fitFrame = 0;
  const refit = (): void => {
    cancelAnimationFrame(fitFrame);
    fitFrame = requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {
        // A host that is not laid out yet has no size to fit to.
      }
    });
  };
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(refit) : null;
  observer?.observe(host);
  fit.fit();

  let disposed = false;
  const pendingDone = new Set<() => void>();
  return {
    write(data, done) {
      if (disposed) return done();
      pendingDone.add(done);
      term.write(data, () => {
        pendingDone.delete(done);
        done();
      });
    },
    size: () => ({ cols: term.cols, rows: term.rows }),
    focus: () => term.focus(),
    hasSelection: () => term.hasSelection(),
    getSelection: () => term.getSelection(),
    paste: (text) => term.paste(text),
    applicationCursor: () => term.modes.applicationCursorKeysMode,
    fit: () => fit.fit(),
    dispose() {
      if (disposed) return;
      disposed = true;
      cancelAnimationFrame(fitFrame);
      observer?.disconnect();
      for (const subscription of subscriptions) subscription.dispose();
      term.dispose();
      // A write that was still being parsed never calls back: let the controller go on.
      for (const done of pendingDone) done();
      pendingDone.clear();
    },
  };
}
