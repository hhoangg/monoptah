// One terminal as the workspace sees it: the controller, the xterm screen (kept alive in a detached element, so a tab that
// is switched away from, or moved to another pane, keeps what is on its screen), the project it was opened for and the
// resume entry. A session lives in a module registry (below), not in a React component: a terminal tab is mounted only
// while it is the active tab of its pane, but its shell and its read loop must outlive that.
//
// Terminal bytes live in memory only (security.md "Terminal"). The resume entry in sessionStorage holds ids and offsets.
import type { LinkConnection } from "@/link";
import { TerminalController, type OpenTarget } from "./controller";
import { linkPortOf } from "./port";
import { sessionResumeStore, type ResumeStore } from "./resume";
import { createXtermScreen, type XtermScreen } from "./xtermScreen";

/** The project a terminal was opened for. `label` is what the title shows ("Project" or "Project/folder"). */
export interface ChosenTarget extends OpenTarget {
  label: string;
}

export type CreateScreen = typeof createXtermScreen;

export class TerminalSession {
  readonly id: string;
  readonly desktopId: string;
  readonly controller: TerminalController;
  /** The element xterm draws into; a view moves it into its container while it is mounted. */
  readonly element: HTMLDivElement;
  readonly store: ResumeStore;
  /** Asks for a passkey ticket (set by the hook that knows this device). */
  #ticketSource: (projectId: string) => Promise<string> = () => Promise.reject(new Error("no ticket source"));
  /** Takes the typed keys of the screen; the view puts the sticky Ctrl and Alt of the key row here. Default: straight to the shell. */
  #onInput: (data: string) => void;
  #target: ChosenTarget | null = null;
  #screen: XtermScreen | null = null;
  #screenPending = false;
  #screenError: string | null = null;
  #beginPending = false;
  #disposed = false;
  readonly #listeners = new Set<() => void>();
  #version = 0;
  readonly #unsubscribe: () => void;

  constructor(desktopId: string, id: string, connection: Pick<LinkConnection, "request" | "store">, store?: ResumeStore) {
    this.id = id;
    this.desktopId = desktopId;
    this.store = store ?? sessionResumeStore(`${desktopId}.${id}`);
    this.controller = new TerminalController({ link: linkPortOf(connection), store: this.store, requestTicket: (projectId) => this.#ticketSource(projectId) });
    this.element = document.createElement("div");
    this.element.className = "h-full w-full";
    this.element.dataset["terminalHost"] = id;
    this.#onInput = (data) => this.controller.input(data);
    this.#unsubscribe = this.controller.subscribe(() => this.#notify());
    // A shell a reload left behind (inside its grace window) is picked up again; it waits for a screen before it reads.
    const saved = this.store.load();
    if (saved) {
      this.#target = { projectId: "", label: saved.cwd };
      this.controller.resume(saved);
    }
  }

  /** How this device asks for a passkey ticket (set by the hook that knows the device). */
  setTicketSource(source: (projectId: string) => Promise<string>): void {
    this.#ticketSource = source;
  }

  /** Where the typed keys go (the view's sticky-modifier path); `null` restores the straight path to the shell. */
  setInput(handler: ((data: string) => void) | null): void {
    this.#onInput = handler ?? ((data) => this.controller.input(data));
  }

  // --- Store for the UI ---

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Changes whenever the target, the screen error or the controller snapshot changes. */
  getVersion = (): number => this.#version;

  #notify(): void {
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
  }

  get target(): ChosenTarget | null {
    return this.#target;
  }

  get screenError(): string | null {
    return this.#screenError;
  }

  get screen(): XtermScreen | null {
    return this.#screen;
  }

  /** The shell is up, coming back or being opened: closing it needs a confirmation. */
  get live(): boolean {
    const { phase, session, ended } = this.controller.getSnapshot();
    return phase === "starting" || (session !== null && ended === null && (phase === "running" || phase === "reconnecting"));
  }

  /** The text of the title and of the footer switcher: the project, or nothing before one is chosen. */
  get project(): string | null {
    const session = this.controller.getSnapshot().session;
    return session?.cwd ?? this.#target?.label ?? null;
  }

  /** The project name for the title: only while a shell is being opened, runs or has ended (not in the chooser). */
  get titleProject(): string | null {
    const { phase } = this.controller.getSnapshot();
    return phase === "failed" || (phase === "idle" && this.#target === null) ? null : this.project;
  }

  /** Green dot: the shell is running and the link is up. */
  get connected(): boolean {
    const { phase, ended } = this.controller.getSnapshot();
    return ended === null && phase === "running";
  }

  // --- Screen ---

  /** Creates the xterm screen once (loaded on demand) and starts a pending open as soon as it exists. */
  ensureScreen(createScreen: CreateScreen = createXtermScreen): void {
    if (this.#screen || this.#screenPending || this.#disposed) return;
    this.#screenPending = true;
    createScreen(this.element, { onData: (data) => this.#onInput(data), onResize: (cols, rows) => this.controller.resize(cols, rows) }).then(
      (made) => {
        this.#screenPending = false;
        if (this.#disposed) return made.dispose();
        this.#screen = made;
        this.controller.attachScreen(made);
        this.#notify();
        this.#flushBegin();
      },
      () => {
        this.#screenPending = false;
        this.#screenError = "The terminal could not be loaded. Reload the page and try again.";
        this.#notify();
      },
    );
  }

  // --- Opening ---

  /** The user chose a project: ask for the passkey and open the shell, as soon as the screen exists (its size is known). */
  open(target: ChosenTarget): void {
    this.controller.reset();
    this.#target = target;
    this.#beginPending = true;
    this.#notify();
    this.#flushBegin();
  }

  #flushBegin(): void {
    const target = this.#target;
    if (!this.#beginPending || !this.#screen || !target || this.#disposed) return;
    this.#beginPending = false;
    void this.controller.begin({ projectId: target.projectId, ...(target.subdir === undefined ? {} : { subdir: target.subdir }) });
  }

  /** Back to the chooser (after an end or a failed open, or a cancelled passkey prompt). */
  backToChooser(): void {
    this.#beginPending = false;
    this.controller.cancel();
    this.controller.reset();
    this.#target = null;
    this.#notify();
  }

  // --- End ---

  /** Kills the shell on the desktop (when there is one) and frees everything. */
  async close(): Promise<void> {
    await this.controller.close();
    this.dispose();
  }

  /**
   * Frees the screen and the loops without killing the shell. By default the resume entry goes too (the terminal was
   * closed); `keepResume` leaves it, so a page that goes away can reattach to the shell within its grace window.
   */
  dispose({ keepResume = false }: { keepResume?: boolean } = {}): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#unsubscribe();
    this.controller.dispose();
    this.#screen?.dispose();
    this.#screen = null;
    this.element.remove();
    if (!keepResume) this.store.clear();
    this.#notify();
  }
}
