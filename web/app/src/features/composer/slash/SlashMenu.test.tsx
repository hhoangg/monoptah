// The menu in the composer with a stand-in controller: the phone sheet, the tap, the closed states.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SlashCommand } from "@monoptah/protocol";
import { Composer, DESKTOP_LAYOUT_QUERY } from "../Composer";
import type { SlashController } from "./useSlashController";

function setMedia(desktop: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query === DESKTOP_LAYOUT_QUERY ? desktop : false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
}

beforeEach(() => {
  setMedia(true);
  window.scrollTo = vi.fn() as never;
});

const row = (id: string, label: string, extra: Partial<SlashCommand> = {}): SlashCommand => ({ id, kind: "builtin", label, detail: `${label} detail`, icon: id, available: true, mode: false, active: false, ...extra });

function controller(commands: SlashCommand[], extra: Partial<SlashController> = {}): SlashController {
  return {
    commands,
    loading: false,
    failed: false,
    refresh: vi.fn(),
    mode: null,
    armed: null,
    choose: vi.fn(() => Promise.resolve()),
    clearMode: vi.fn(() => Promise.resolve()),
    disarm: vi.fn(),
    submitArmed: vi.fn(() => Promise.resolve(false)),
    prefix: (text) => text,
    afterSend: vi.fn(),
    addToFolder: false,
    ...extra,
  };
}

describe("the / menu inside the composer", () => {
  it("is a bottom sheet on a phone layout and a tap on a row chooses it", async () => {
    setMedia(false);
    const user = userEvent.setup();
    const slash = controller([row("plan", "Plan mode", { mode: true }), row("compact", "Compact context")]);
    render(<Composer onSubmit={() => Promise.resolve()} slash={slash} />);
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "/");
    expect(screen.getByTestId("slash-menu")).toHaveAttribute("data-layout", "sheet");
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: /Plan mode/ }));
    expect(slash.choose).toHaveBeenCalledWith(expect.objectContaining({ id: "plan" }));
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
    expect((field as HTMLTextAreaElement).value).toBe("");
    // A phone's Enter stays a newline: only a tap or Tab chooses.
    await user.type(field, "/c{Enter}");
    expect(slash.choose).toHaveBeenCalledTimes(1);
  });

  it("opens nothing and takes no key when the desktop has no commands, and says so when the read failed", async () => {
    const user = userEvent.setup();
    const empty = controller([]);
    const { rerender } = render(<Composer onSubmit={() => Promise.resolve()} slash={empty} />);
    const field = screen.getByRole("textbox", { name: "Message" });
    await user.type(field, "/");
    expect(screen.queryByRole("listbox")).toBeNull();
    await user.keyboard("{Escape}");
    expect(empty.refresh).not.toHaveBeenCalled();
    rerender(<Composer onSubmit={() => Promise.resolve()} slash={controller([], { failed: true })} />);
    await user.clear(field);
    await user.type(field, "/");
    expect(screen.getByRole("listbox")).toHaveTextContent("Could not load the commands");
  });

  it("asks the desktop for the rows each time it opens", async () => {
    const user = userEvent.setup();
    const slash = controller([row("plan", "Plan mode")]);
    render(<Composer onSubmit={() => Promise.resolve()} slash={slash} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
    expect(slash.refresh).toHaveBeenCalledTimes(1);
  });

  it("stays closed without a controller, for a path, and in a read-only composer", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<Composer onSubmit={() => Promise.resolve()} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
    expect(screen.queryByRole("listbox")).toBeNull();
    const slash = controller([row("plan", "Plan mode")]);
    rerender(<Composer onSubmit={() => Promise.resolve()} slash={slash} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), " src/a.ts /usr/b");
    expect(screen.queryByRole("listbox")).toBeNull();
    rerender(<Composer onSubmit={() => Promise.resolve()} slash={slash} readOnly />);
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("shows an unavailable row with its reason, and Enter on it keeps the menu and says why", async () => {
    const user = userEvent.setup();
    const slash = controller([row("compact", "Compact context", { available: false, reason: "Send a message before compacting this session" })]);
    render(<Composer onSubmit={() => Promise.resolve()} slash={slash} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
    expect(screen.getByRole("option")).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("option")).toHaveTextContent("Send a message before compacting");
    await user.keyboard("{Enter}");
    expect(slash.choose).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Send a message before compacting");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
  });

  it("an armed Draft turns Send into Save draft and sends through the controller, not the page", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn(() => Promise.resolve());
    const submitArmed = vi.fn(() => Promise.resolve(true));
    const slash = controller([row("draft", "Draft", { mode: true })], { armed: row("draft", "Draft", { mode: true }), submitArmed });
    render(<Composer onSubmit={onSubmit} slash={slash} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "later");
    await user.click(screen.getByRole("button", { name: "Save draft" }));
    await waitFor(() => expect(submitArmed).toHaveBeenCalledWith("later"));
    expect(onSubmit).not.toHaveBeenCalled();
    expect(slash.afterSend).toHaveBeenCalled();
  });
  // The real /d frame: `main` holds a top bar that does not clip and, under it, the chat pane scroller whose top is the
  // bar's bottom. `main` itself scrolls too, but its top is 0, so it does not narrow the room.
  function phoneFrame(vvHeight: number, composer: [number, number], pane: [number, number]) {
    const original = Object.getOwnPropertyDescriptor(window, "visualViewport");
    const listeners = new Map<string, Set<() => void>>();
    const state = { vvHeight, composer, pane };
    Object.defineProperty(window, "visualViewport", {
      configurable: true,
      value: {
        pageTop: 0,
        offsetTop: 0,
        scale: 1,
        get height() { return state.vvHeight; },
        addEventListener: (type: string, fn: () => void) => void (listeners.get(type) ?? listeners.set(type, new Set()).get(type)!).add(fn),
        removeEventListener: (type: string, fn: () => void) => void listeners.get(type)?.delete(fn),
      },
    });
    const rect = (top: number, bottom: number) => ({ top, bottom, left: 0, right: 390, width: 390, height: bottom - top, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      if (this.tagName === "MAIN") return rect(0, 844);
      if (this.getAttribute("data-testid") === "chat-pane") return rect(...state.pane);
      if (this.getAttribute("data-testid") === "composer") return rect(...state.composer);
      return rect(0, 0);
    });
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((fn) => (fn(0), 1));
    return {
      resize: (height: number) => {
        state.vvHeight = height;
        state.pane = [state.pane[0], height];
        act(() => listeners.get("resize")?.forEach((fn) => fn()));
      },
      restore: () => {
        vi.restoreAllMocks();
        if (original) Object.defineProperty(window, "visualViewport", original);
        else Reflect.deleteProperty(window, "visualViewport");
      },
    };
  }

  function renderPhone(count = 2) {
    setMedia(false);
    const rows = Array.from({ length: count }, (_, index) => row(`c${index}`, `Command ${index}`));
    return render(
      <main style={{ overflowY: "auto" }}>
        <div data-testid="top-bar" />
        <div data-testid="chat-pane" style={{ overflowY: "auto" }}>
          <Composer onSubmit={() => Promise.resolve()} slash={controller(rows)} />
        </div>
      </main>,
    );
  }

  it("fits the room between the top bar and the composer when the keyboard is open (390x844 phone)", async () => {
    const frame = phoneFrame(500, [330, 480], [60, 500]);
    try {
      const user = userEvent.setup();
      renderPhone();
      await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
      const menu = screen.getByTestId("slash-menu");
      // 330 (composer top) - 8 (gap) - 68 (pane top 60 + margin 8) = 254, under the 55% cap (275).
      expect(menu).toHaveStyle({ maxHeight: "254px" });
      expect(menu).toHaveAttribute("data-side", "above");
      expect(menu).toHaveClass("bottom-full");
      expect(menu).toHaveAttribute("data-menu-surface");
    } finally {
      frame.restore();
    }
  });

  it("caps the phone sheet at 55% of the visual viewport, not of dvh, when there is more room", async () => {
    const frame = phoneFrame(500, [600, 700], [60, 844]);
    try {
      const user = userEvent.setup();
      renderPhone();
      await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
      expect(screen.getByTestId("slash-menu")).toHaveStyle({ maxHeight: `${Math.floor(0.55 * 500)}px` });
    } finally {
      frame.restore();
    }
  });

  it("flips below the composer when there is no room above it", async () => {
    const frame = phoneFrame(844, [70, 150], [60, 844]);
    try {
      const user = userEvent.setup();
      renderPhone();
      await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
      const menu = screen.getByTestId("slash-menu");
      expect(menu).toHaveAttribute("data-side", "below");
      expect(menu).toHaveClass("top-full");
      expect(menu).not.toHaveClass("bottom-full");
      expect(menu).toHaveStyle({ maxHeight: `${Math.floor(0.55 * 844)}px` });
    } finally {
      frame.restore();
    }
  });

  it("measures again when the keyboard opens while the menu is open", async () => {
    const frame = phoneFrame(844, [700, 800], [60, 844]);
    try {
      const user = userEvent.setup();
      renderPhone();
      await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
      const menu = screen.getByTestId("slash-menu");
      expect(menu).toHaveStyle({ maxHeight: `${Math.floor(0.55 * 844)}px` });
      frame.resize(500);
      await waitFor(() => expect(menu).toHaveStyle({ maxHeight: "275px" }));
    } finally {
      frame.restore();
    }
  });

  it("keeps the active row in view with the arrow keys by scrolling the list, not the page", async () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get(this: HTMLElement) { return this.getAttribute("role") === "listbox" ? 80 : 0; } });
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const make = (top: number, height: number) => ({ top, bottom: top + height, left: 0, right: 300, width: 300, height, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
      if (this.getAttribute("role") === "option") {
        const list = this.parentElement as HTMLElement;
        return make(100 + Array.from(list.children).indexOf(this) * 40 - list.scrollTop, 40);
      }
      return this.getAttribute("role") === "listbox" ? make(100, 80) : make(0, 0);
    });
    try {
      const user = userEvent.setup();
      const rows = ["a", "b", "c", "d", "e"].map((id) => row(id, `Command ${id}`));
      const { rerender } = render(<Composer onSubmit={() => Promise.resolve()} slash={controller(rows)} />);
      await user.type(screen.getByRole("textbox", { name: "Message" }), "/");
      const list = screen.getByRole("listbox");
      expect(list.scrollTop).toBe(0);
      await user.keyboard("{ArrowDown}{ArrowDown}");
      // The third row spans 80..120 in a list that shows 0..80.
      await waitFor(() => expect(list.scrollTop).toBe(40));
      expect(scrollIntoView).not.toHaveBeenCalled();
      // The user scrolls the list by hand; a re-render with an equal (but new) rows array must not snap it back.
      list.scrollTop = 0;
      rerender(<Composer onSubmit={() => Promise.resolve()} slash={controller([...rows])} />);
      expect(screen.getByRole("listbox").scrollTop).toBe(0);
      await user.keyboard("{ArrowUp}{ArrowUp}");
      await waitFor(() => expect(list.scrollTop).toBe(0));
    } finally {
      vi.restoreAllMocks();
      if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
      else Reflect.deleteProperty(HTMLElement.prototype, "clientHeight");
      Reflect.deleteProperty(Element.prototype, "scrollIntoView");
    }
  });
});
