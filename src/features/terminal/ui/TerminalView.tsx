import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef } from "react";
import {
  getPtyStatus,
  holdPtyBridge,
  killPty,
  resizePty,
  spawnPty,
  subscribePty,
  writePty,
  type PtyLaunch,
} from "../../../platform/tauri/pty";
import {
  claimPtyArrival,
  consumePtyHandoff,
  takeArrivedExit,
  type PtyHandoffExit,
} from "../model/ptyHandoff";
import { isOscColorQuery, oscColorReply } from "../model/terminalChrome";
import {
  isMacTerminalClearShortcut,
  macTerminalShortcutData,
  multiCharacterKeyText,
} from "../model/terminalKeys";
import {
  defaultTerminalTitle,
  scanOscCwd,
  type TerminalMetaPatch,
} from "../model/terminalTab";
import { isLightScheme, SCHEME_CHANGE_EVENT } from "../../settings/model/appearance";
import {
  applyTerminalChrome,
  fitTerminal,
  type TerminalFitMode,
} from "../model/terminalLayout";
import { IS_MAC } from "../../../platform/tauri/platform";
import "@xterm/xterm/css/xterm.css";

type Props = {
  id: string;
  cwd: string;
  active: boolean;
  /** Runs this program instead of the login shell. Read once per spawn. */
  launch?: PtyLaunch;
  /**
   * This pane was handed off from another view and its process is still
   * running: attach to it and never spawn. Left unset, the view takes the
   * answer from the handoff record for its id. A caller that mounts the view
   * later than it mounts itself (after async work) decides at its own mount
   * and passes the answer here.
   */
  attach?: boolean;
  onMetaChange?: (patch: TerminalMetaPatch) => void;
  /** Reports the process ending, or failing to start (`error`, no code). */
  onExit?: (exit: {
    code: number | null;
    error?: string;
    /** Last output the process wrote, for telling why it stopped. */
    output?: string;
  }) => void;
  /** Collects the output tail for `onExit`. Off by default: it decodes every chunk. */
  captureExitOutput?: boolean;
  /** Window title the program sets (OSC 0/2). Shells are left alone unless set. */
  onTitleChange?: (title: string) => void;
  /** Called once the process has actually been spawned. */
  onSpawn?: () => void;
};

function cssColor(expr: string, fallback: string): string {
  const probe = document.createElement("span");
  probe.style.color = expr;
  document.body.appendChild(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color || fallback;
}

function cssHexColor(expr: string, fallback: string): string {
  const color = cssColor(expr, fallback);
  if (/^#[\da-f]{6}$/i.test(color)) return color;
  const channels = color.match(/[\d.]+/g)?.slice(0, 3).map(Number);
  if (!channels || channels.length < 3 || channels.some(Number.isNaN)) {
    return fallback;
  }
  return `#${channels
    .map((channel) =>
      Math.round(Math.min(255, Math.max(0, channel)))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

const ANSI_DARK = {
  black: "#1d2428",
  red: "#f87171",
  green: "#4ade80",
  yellow: "#fbbf24",
  blue: "#60a5fa",
  magenta: "#c084fc",
  cyan: "#22d3ee",
  white: "#e8eef2",
  brightBlack: "#64748b",
  brightRed: "#fca5a5",
  brightGreen: "#86efac",
  brightYellow: "#fde68a",
  brightBlue: "#93c5fd",
  brightMagenta: "#d8b4fe",
  brightCyan: "#67e8f9",
  brightWhite: "#f8fafc",
};

// One-Light-family palette tuned for a near-white canvas.
const ANSI_LIGHT = {
  black: "#383a42",
  red: "#e45649",
  green: "#50a14f",
  yellow: "#c18401",
  blue: "#4078f2",
  magenta: "#a626a4",
  cyan: "#0184bc",
  white: "#fafafa",
  brightBlack: "#7c8591",
  brightRed: "#df6b60",
  brightGreen: "#68b567",
  brightYellow: "#d19a2f",
  brightBlue: "#5c89f5",
  brightMagenta: "#b54bb3",
  brightCyan: "#1f9cc9",
  brightWhite: "#ffffff",
};

function terminalTheme(light: boolean) {
  return {
    background: "#00000000",
    foreground: cssColor("var(--color-content)", light ? "#2e2e2e" : "#e8eef2"),
    cursor: cssColor("var(--color-accent)", light ? "#4078f2" : "#4da3f5"),
    cursorAccent: light ? "#ffffff" : "#000000",
    selectionBackground: light ? "rgba(0,0,0,0.18)" : "rgba(255,255,255,0.18)",
    selectionInactiveBackground: light
      ? "rgba(0,0,0,0.08)"
      : "rgba(255,255,255,0.08)",
    ...(light ? ANSI_LIGHT : ANSI_DARK),
  };
}

function terminalFont(): string {
  const fromCss = getComputedStyle(document.documentElement)
    .getPropertyValue("--font-terminal")
    .trim();
  return fromCss || "ui-monospace, SFMono-Regular, Menlo, Monaco, monospace";
}

/**
 * Teardown still in flight per PTY id. A view that mounts with an id another
 * view (or a StrictMode replay of itself) is still stopping must wait, or the
 * late kill lands on the replacement shell and drops its data handler.
 */
const stoppingPtys = new Map<string, Promise<void>>();

/**
 * Longest a handed-off view's pending start may hold the next view back. The
 * wait covers a spawn that is a few milliseconds from landing; a start that
 * never settles (an IPC that never answers) must not leave the pane blank.
 */
const HANDOFF_WAIT_MS = 5000;

/** Enough to hold a CLI's one-line failure message. */
const OUTPUT_TAIL_CHARS = 4000;

function oscColors() {
  const light = isLightScheme();
  return {
    fg: cssHexColor(
      "var(--color-content)",
      light ? "#2e2e2e" : "#ebebeb",
    ),
    bg: cssHexColor(
      "var(--color-background-base)",
      light ? "#f7f7f7" : "#171717",
    ),
    cursor: cssHexColor(
      "var(--color-accent)",
      light ? "#4078f2" : "#4da3f5",
    ),
  };
}

/**
 * Open an HTTP(S) terminal link in the default browser, ignoring other schemes
 * and opener failures.
 */
function openTerminalLink(uri: string) {
  if (!/^https?:\/\//i.test(uri)) return;
  void openUrl(uri).catch(() => undefined);
}

export function TerminalView({
  id,
  cwd,
  active,
  launch,
  attach: attachProp,
  onMetaChange,
  onExit,
  captureExitOutput,
  onTitleChange,
  onSpawn,
}: Props) {
  const outerRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const spawned = useRef(false);
  const applySizeRef = useRef<() => void>(() => {});
  const onMetaChangeRef = useRef(onMetaChange);
  onMetaChangeRef.current = onMetaChange;
  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;
  const captureExitOutputRef = useRef(captureExitOutput);
  captureExitOutputRef.current = captureExitOutput;
  const onSpawnRef = useRef(onSpawn);
  onSpawnRef.current = onSpawn;
  const onTitleChangeRef = useRef(onTitleChange);
  onTitleChangeRef.current = onTitleChange;
  // The spawn effect is keyed on `id` alone, so a new launch object must not
  // restart the process; it is read when the spawn happens.
  const launchRef = useRef(launch);
  launchRef.current = launch;
  const attachPropRef = useRef(attachProp);
  attachPropRef.current = attachProp;
  // Decided once per id and kept across a StrictMode replay of the effect: the
  // handoff record is consumed by the first run, and the replay must not read
  // its absence as "spawn".
  const intentRef = useRef<{
    id: string;
    attach: boolean;
    exit: PtyHandoffExit | undefined;
  } | null>(null);
  const runningProcessRef = useRef<string | null>(null);

  useEffect(() => {
    const outer = outerRef.current;
    const host = hostRef.current;
    if (!outer || !host) return;

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: "bar",
      fontFamily: terminalFont(),
      fontSize: 13,
      lineHeight: 1,
      letterSpacing: 0,
      scrollback: 5000,
      allowTransparency: true,
      smoothScrollDuration: 0,
      theme: terminalTheme(isLightScheme()),
      macOptionIsMeta: IS_MAC,
      linkHandler: { activate: (_event, uri) => openTerminalLink(uri) },
    });
    term.loadAddon(new WebLinksAddon((_event, uri) => openTerminalLink(uri)));
    term.open(host);
    termRef.current = term;
    let closed = false;
    if (intentRef.current?.id !== id) {
      intentRef.current = {
        id,
        attach: attachPropRef.current ?? claimPtyArrival(id),
        exit: takeArrivedExit(id),
      };
    }
    const attach = intentRef.current.attach;
    const arrivedExit = intentRef.current.exit;

    const onCopy = (event: ClipboardEvent) => {
      const text = term.getSelection();
      if (!text) return;
      event.clipboardData?.setData("text/plain", text);
      event.preventDefault();
    };
    const onPaste = (event: ClipboardEvent) => {
      const text = event.clipboardData?.getData("text/plain");
      if (!text) return;
      event.preventDefault();
      term.paste(text);
    };
    host.addEventListener("copy", onCopy);
    host.addEventListener("paste", onPaste);

    term.attachCustomKeyEventHandler((event) => {
      const shortcutData = IS_MAC ? macTerminalShortcutData(event) : null;
      if (shortcutData) {
        if (event.type === "keydown") {
          event.preventDefault();
          event.stopPropagation();
          term.input(shortcutData);
        }
        return false;
      }
      if (IS_MAC && isMacTerminalClearShortcut(event)) {
        if (event.isComposing) return false;
        if (event.type === "keydown") {
          event.preventDefault();
          term.clear();
        }
        return false;
      }

      // An input method may hand over a whole string as one keydown; xterm
      // would emit only its first character.
      const text = multiCharacterKeyText(event);
      if (text) {
        if (event.type === "keydown") {
          // Also stops the browser from following up with beforeinput/input.
          event.preventDefault();
          term.input(text);
        }
        return false;
      }

      const mod = event.metaKey || event.ctrlKey;
      if (!mod || event.altKey) return true;
      const key = event.key.toLowerCase();
      if (key === "c") {
        if (term.hasSelection()) return false;
        if (event.metaKey && !event.ctrlKey) return false;
        return true;
      }
      if (key === "v") return false;
      return true;
    });

    // Only a view that asked for titles listens, so shell tabs keep the
    // process-name titles they already get.
    const titleSub = onTitleChangeRef.current
      ? term.onTitleChange((title) => onTitleChangeRef.current?.(title))
      : null;

    let oscBuffer = "";
    // Kept only for a view that opted in with `captureExitOutput`, so shells
    // and file-pane terminals do not decode every chunk twice.
    let outputTail = "";
    const outputDecoder = new TextDecoder();

    let unsubscribe = () => {};
    let didStart = false;
    let didSpawn = false;
    let exitSeen = false;
    // The exit this view told its owner about, handed on if the pane moves so
    // the next view shows it instead of failing to reattach to a dead process.
    let reported: PtyHandoffExit | null = null;
    // Set when the cleanup left the process running for the next view.
    let handedOff = false;
    // Settles once the terminal has been fitted to its pane (or closed), so an
    // attach can ask the program to repaint at the size it will keep.
    let fitDone = () => {};
    const fitted = new Promise<void>((resolve) => {
      fitDone = resolve;
    });
    const start = async () => {
      if (closed) return;
      // Subscribing first keeps the event bridge held across a handoff, and
      // buffers anything the process says while the rest of start runs.
      unsubscribe = subscribePty(
        id,
        (data) => {
          if (closed) return;
          if (captureExitOutputRef.current) {
            outputTail = (
              outputTail + outputDecoder.decode(data, { stream: true })
            ).slice(-OUTPUT_TAIL_CHARS);
          }
          const onMeta = onMetaChangeRef.current;
          if (onMeta) {
            const text = new TextDecoder().decode(data);
            const scanned = scanOscCwd(text, oscBuffer);
            oscBuffer = scanned.rest;
            if (scanned.cwd) {
              const patch: TerminalMetaPatch = { cwd: scanned.cwd };
              if (!runningProcessRef.current) {
                patch.title = defaultTerminalTitle(scanned.cwd);
              }
              onMeta(patch);
            }
          }
          term.write(data);
        },
        (code) => {
          if (closed) return;
          exitSeen = true;
          reported = { code: code ?? null };
          const status = code == null ? "" : ` (${code})`;
          term.writeln(`\r\n[process exited${status}]`);
          onExitRef.current?.({
            code: code ?? null,
            ...(captureExitOutputRef.current ? { output: outputTail } : {}),
          });
        },
      );
      didStart = true;
      if (attach && arrivedExit) {
        // The process had already ended when the pane moved, and the view that
        // left reported how. Say the same again: the process is not "lost", and
        // the owner keeps the real code and its restart affordances.
        exitSeen = true;
        reported = arrivedExit;
        if (arrivedExit.error) {
          term.writeln(`\x1b[31m${arrivedExit.error}\x1b[0m`);
        } else {
          const status =
            arrivedExit.code == null ? "" : ` (${arrivedExit.code})`;
          term.writeln(`\r\n[process exited${status}]`);
        }
        onExitRef.current?.({
          code: arrivedExit.code,
          ...(arrivedExit.error ? { error: arrivedExit.error } : {}),
        });
        return;
      }
      if (attach) {
        // This pane was moved and its process is still running. Spawning
        // would kill it, so a pane that meant to attach never spawns: if the
        // process is gone, or cannot be confirmed, it says so instead.
        try {
          await getPtyStatus(id);
        } catch (error) {
          // The exit notice has already told the user what happened.
          if (exitSeen) return;
          const reason = error instanceof Error ? error.message : String(error);
          throw new Error(
            `Could not reattach to the running terminal (${reason}). Restart it to continue.`,
          );
        }
        if (closed) return;
        // Replay cannot rebuild a full-screen program's display; a resize
        // signals it to repaint into this fresh terminal. The kernel only
        // signals when the size changes, and the fitted size can equal the
        // old one, so step off it and back, from the real size.
        await fitted;
        if (closed) return;
        try {
          await resizePty(id, term.cols, Math.max(2, term.rows - 1));
          await resizePty(id, term.cols, term.rows);
        } catch {
          // A failed repaint nudge must not fail the pane: the process is
          // fine. Let the next fit send the size again.
          lastCols = 0;
          lastRows = 0;
          schedule();
        }
        return;
      }
      const launchNow = launchRef.current;
      await (launchNow
        ? spawnPty(id, cwd, term.cols, term.rows, launchNow)
        : spawnPty(id, cwd, term.cols, term.rows));
      didSpawn = true;
    };

    // `start` always runs behind a `.then`, never synchronously. That is what
    // makes a StrictMode replay safe after a handoff: the replay's cleanup runs
    // before this view's `start` does, so `closed` is already true, `didStart`
    // never gets set, and the stop path below kills nothing. If `start` ever
    // ran inline, that cleanup would kill the process just handed over.
    const starting = (stoppingPtys.get(id) ?? Promise.resolve())
      .then(start)
      .then(() => {
        if (closed) {
          // A pane moved while its spawn was in flight still started: the next
          // view attaches and reports nothing, so the start is reported here.
          if (handedOff && didSpawn) onSpawnRef.current?.();
          return;
        }
        spawned.current = true;
        // Nothing was launched when attaching, so there is no start to report.
        if (didSpawn) onSpawnRef.current?.();
      })
      .catch((error) => {
        spawned.current = false;
        if (!closed) {
          const message =
            error instanceof Error ? error.message : String(error);
          reported = { code: null, error: message };
          term.writeln(`\x1b[31m${message}\x1b[0m`);
          onExitRef.current?.({ code: null, error: message });
        }
        throw error;
      });
    void starting.catch(() => undefined);

    const dataSub = term.onData((data) => {
      void starting
        .then(() => (closed ? undefined : writePty(id, data)))
        .catch(() => undefined);
    });

    const replyOsc = (code: 10 | 11 | 12, hex: string) => {
      const reply = oscColorReply(code, hex);
      if (reply) {
        void starting
          .then(() => (closed ? undefined : writePty(id, reply)))
          .catch(() => undefined);
      }
      return true;
    };
    const oscFg = term.parser.registerOscHandler(10, (data) =>
      isOscColorQuery(data) ? replyOsc(10, oscColors().fg) : false,
    );
    const oscBg = term.parser.registerOscHandler(11, (data) =>
      isOscColorQuery(data) ? replyOsc(11, oscColors().bg) : false,
    );
    const oscCursor = term.parser.registerOscHandler(12, (data) =>
      isOscColorQuery(data) ? replyOsc(12, oscColors().cursor) : false,
    );

    const onSchemeChange = () => {
      term.options.theme = terminalTheme(isLightScheme());
    };
    window.addEventListener(SCHEME_CHANGE_EVENT, onSchemeChange);

    term.attachCustomWheelEventHandler(() => {
      if (term.element?.classList.contains("enable-mouse-events")) return true;
      return term.buffer.active.type !== "alternate";
    });

    let lastCols = 0;
    let lastRows = 0;
    let raf = 0;
    let tuiMode = false;

    const fitMode = (): TerminalFitMode =>
      term.buffer.active.type === "alternate" ? "tui" : "shell";

    const syncAltScreenMode = () => {
      const next = fitMode() === "tui";
      if (next === tuiMode) return;
      tuiMode = next;
      applyTerminalChrome(term, outer, next);
      lastCols = 0;
      lastRows = 0;
      schedule();
    };

    const applySize = () => {
      if (closed) return;
      const next = fitTerminal(term, host, fitMode());
      if (!next) return;
      const { cols, rows } = next;
      fitDone();
      if (cols === lastCols && rows === lastRows) return;
      lastCols = cols;
      lastRows = rows;
      void starting
        .then(() => (closed ? undefined : resizePty(id, cols, rows)))
        .catch(() => {
          lastCols = 0;
          lastRows = 0;
        });
    };

    const schedule = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        applySize();
      });
    };

    applySizeRef.current = applySize;
    const renderSub = term.onRender(() => {
      if (!spawned.current) applySize();
    });
    const bufferSub = term.buffer.onBufferChange(syncAltScreenMode);
    syncAltScreenMode();
    const frame = requestAnimationFrame(applySize);
    const observer = new ResizeObserver(schedule);
    observer.observe(host);

    return () => {
      closed = true;
      fitDone();
      cancelAnimationFrame(frame);
      if (raf) cancelAnimationFrame(raf);
      observer.disconnect();
      outer.classList.remove("monocode-terminal--alt-screen");
      applySizeRef.current = () => {};
      host.removeEventListener("copy", onCopy);
      host.removeEventListener("paste", onPaste);
      window.removeEventListener(SCHEME_CHANGE_EVENT, onSchemeChange);
      dataSub.dispose();
      oscFg.dispose();
      oscBg.dispose();
      oscCursor.dispose();
      renderSub.dispose();
      bufferSub.dispose();
      titleSub?.dispose();
      if (consumePtyHandoff(id, reported ?? undefined)) {
        handedOff = true;
        // The pane is moving, not closing: leave the process running for the
        // view that mounts next and never kill it. That view still waits for
        // a spawn this one has in flight, or it would see no process yet and
        // spawn a second one over the first. Output buffers meanwhile and
        // replays on its subscribe.
        //
        // Two things keep that wait from outliving its purpose. The event
        // bridge is held until it ends, so dropping this view's subscription
        // cannot arm its teardown and lose the output the next view replays.
        // And the wait is bounded, so a start that never settles delays the
        // next view instead of blocking it for good.
        const releaseBridge = holdPtyBridge();
        const parked = new Promise<void>((resolve) => {
          const timer = setTimeout(resolve, HANDOFF_WAIT_MS);
          const settle = () => {
            clearTimeout(timer);
            resolve();
          };
          starting.then(settle, settle);
        }).finally(() => {
          releaseBridge();
          if (stoppingPtys.get(id) === parked) stoppingPtys.delete(id);
        });
        stoppingPtys.set(id, parked);
        unsubscribe();
        term.dispose();
        termRef.current = null;
        spawned.current = false;
        return;
      }
      const stopping = starting
        .catch(() => undefined)
        .then(() => {
          unsubscribe();
          // `didStart` stays false when this view closed before `start` ran
          // (see the note on `starting`), so such a view never kills.
          return didStart ? killPty(id) : undefined;
        })
        .finally(() => {
          if (stoppingPtys.get(id) === stopping) stoppingPtys.delete(id);
        });
      stoppingPtys.set(id, stopping);
      term.dispose();
      termRef.current = null;
      spawned.current = false;
    };
  }, [id]);

  // Identity-stable: the callers pass an inline arrow, so depending on the
  // prop itself would tear down and re-arm the poll — and re-fork `ps` — on
  // every parent render.
  const wantsMeta = !!onMetaChange;

  useEffect(() => {
    if (!wantsMeta) return;
    let lastForeground: string | null = null;
    let inFlight = false;
    const refresh = () => {
      if (!spawned.current) return;
      // Each status read forks `ps`; an off-screen window has no title to paint.
      if (document.hidden) return;
      if (inFlight) return;
      inFlight = true;
      void getPtyStatus(id)
        .then(({ foreground }) => {
          const fg = foreground?.trim() || null;
          runningProcessRef.current = fg;
          if (fg === lastForeground) return;
          lastForeground = fg;
          onMetaChangeRef.current?.(
            fg
              ? { title: fg, foreground: fg }
              : { title: defaultTerminalTitle(cwd), foreground: null },
          );
        })
        .catch(() => undefined)
        .finally(() => {
          inFlight = false;
        });
    };
    refresh();
    const interval = setInterval(refresh, 1000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      clearInterval(interval);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [id, cwd, wantsMeta]);

  useEffect(() => {
    if (!active) return;
    applySizeRef.current();
    termRef.current?.focus();
  }, [active]);

  return (
    <div
      ref={outerRef}
      className="monocode-terminal flex h-full w-full min-h-0 min-w-0 flex-col"
      onMouseDown={() => termRef.current?.focus()}
    >
      <div
        ref={hostRef}
        className="monocode-terminal-host min-h-0 min-w-0 flex-1 overflow-hidden"
      />
    </div>
  );
}
