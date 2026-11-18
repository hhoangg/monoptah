import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { TranscriptActionsContext, type TranscriptActions } from "./context";
import { ModelNamesContext } from "./modelNames";
import { resetCollapsedToolCalls, setCollapsedToolCalls, TOOL_CALLS_KEY } from "./toolCallsSetting";
import { TranscriptTurn } from "./TranscriptTurn";

beforeAll(() => {
  window.matchMedia ??= ((query: string) => ({ matches: false, media: query, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.clear();
  resetCollapsedToolCalls();
});
afterEach(() => vi.restoreAllMocks());

function show(turn: Turn, actions: TranscriptActions = {}, agent?: { name: string; color?: string }) {
  return render(
    <TranscriptActionsContext value={actions}>
      <TranscriptTurn turn={turn} {...(agent ? { agent } : {})} />
    </TranscriptActionsContext>,
  );
}

describe("user messages (F-35)", () => {
  it("shows the text, the attachment count and the clock time", () => {
    const at = new Date(2026, 0, 2, 15, 4).getTime();
    show({ id: "t", items: [{ type: "user", id: "u", text: "Please fix it", at, attachments: 2 }] });
    expect(screen.getByText("Please fix it")).toBeInTheDocument();
    expect(screen.getByText("2 attachments")).toBeInTheDocument();
    expect(screen.getByText(/3:04/)).toBeInTheDocument();
  });

  it("says 1 attachment for one", () => {
    show({ id: "t", items: [{ type: "user", id: "u", text: "x", attachments: 1 }] });
    expect(screen.getByText("1 attachment")).toBeInTheDocument();
  });

  it("clamps to four lines and offers Show more when the text is cut", async () => {
    const user = userEvent.setup();
    const scrollHeight = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(230);
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(92);
    show({ id: "t", items: [{ type: "user", id: "u", text: "long\n".repeat(10), at: 1 }] });
    const toggle = screen.getByRole("button", { name: "Show more" });
    await user.click(toggle);
    expect(screen.getByRole("button", { name: "Show less" })).toBeInTheDocument();
    scrollHeight.mockRestore();
  });

  it("shows markup in a message as plain text", () => {
    const { container } = show({ id: "t", items: [{ type: "user", id: "u", text: '<img src=x onerror="alert(1)"> **bold**' }] });
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText(/<img src=x/)).toBeInTheDocument();
  });

  it("puts a lone emoji in a Mono's chat without a bubble, with its reaction", () => {
    const { container } = show({ id: "t", items: [{ type: "user", id: "u", text: " 🎉 ", emojiOnly: true, reaction: "👍" }] }, {}, { name: "Mono" });
    expect(container.querySelector(".bg-bubble")).toBeNull();
    expect(screen.getByRole("img", { name: "Reacted 👍" })).toBeInTheDocument();
  });
});

describe("replies and day separators", () => {
  it("renders a reply as markdown inside an assistant message", () => {
    const { container } = show({ id: "t", items: [{ type: "text", id: "a", text: "Hello **world**" }] });
    expect(container.querySelector('[data-chat-message-role="assistant"]')).not.toBeNull();
    expect(screen.getByText("world").tagName).toBe("STRONG");
  });

  it("shows a day separator with the day and time", () => {
    const at = Date.now();
    show({ id: "t", stampAt: at, items: [{ type: "text", id: "a", text: "x" }] });
    expect(screen.getByRole("separator").getAttribute("aria-label")).toMatch(/^Today /);
  });

  it("renders nothing for an item type it does not know (F-34)", () => {
    const { container } = show({ id: "t", items: [{ type: "hologram", id: "h", text: "secret" }, { type: "text", id: "a", text: "kept" }] });
    expect(container).not.toHaveTextContent("secret");
    expect(screen.getByText("kept")).toBeInTheDocument();
  });
});

describe("fold line (F-36)", () => {
  const folded: Turn = {
    id: "t",
    items: [
      {
        type: "header",
        id: "h",
        live: false,
        durationMs: 72_000,
        model: "Claude",
        harness: "claude",
        entries: [
          { type: "prose", id: "e1", text: "I will look at the parser." },
          { type: "phase", id: "e2", kind: "research", title: "Read 3 files", steps: [{ id: "s1", label: "src/a.ts" }] },
        ],
      },
    ],
  };

  it("labels the finished work and opens its notes and phases", async () => {
    const user = userEvent.setup();
    show(folded);
    const toggle = screen.getByRole("button", { name: "Show the work" });
    expect(screen.getByText("Claude worked for 1m 12s")).toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(screen.getByRole("button", { name: "Hide the work" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("I will look at the parser.")).toBeVisible();
    expect(screen.getByText("Read 3 files")).toBeInTheDocument();
  });

  it("is a plain label when there is nothing to open", () => {
    show({ id: "t", items: [{ type: "header", id: "h", live: false, durationMs: 5000, entries: [] }] });
    expect(screen.getByRole("button", { name: "Worked for 5s" })).toBeDisabled();
  });

  it("shows the catalog's display name for a model id and the raw id when the catalog does not know it", () => {
    const names: Record<string, string> = { "claude:opus": "Claude Opus" };
    const turn = (id: string, model: string): Turn => ({ id, items: [{ type: "header", id: `h${id}`, live: false, durationMs: 72_000, model, entries: [] }] });
    render(
      <ModelNamesContext value={(id) => names[id] ?? id}>
        <TranscriptTurn turn={turn("a", "claude:opus")} />
        <TranscriptTurn turn={turn("b", "mystery-model")} />
      </ModelNamesContext>,
    );
    expect(screen.getByText("Claude Opus worked for 1m 12s")).toBeInTheDocument();
    expect(screen.getByText("mystery-model worked for 1m 12s")).toBeInTheDocument();
  });

  it("ticks while live and shows the waiting label when there is one", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 0, 1, 12, 0, 0));
    try {
      show({ id: "t", items: [{ type: "header", id: "h", live: true, startedAt: Date.now() - 5000, model: "Claude", entries: [] }] });
      expect(screen.getByText("Claude working for 5s")).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(3000));
      expect(screen.getByText("Claude working for 8s")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    show({ id: "t2", items: [{ type: "header", id: "h2", live: true, waiting: "Waiting for approval", entries: [] }] });
    expect(screen.getByText("Waiting for approval")).toBeInTheDocument();
  });
});

describe("tool phases (F-37)", () => {
  const steps = ["a", "b", "c", "d", "e", "f"].map((label) => ({ id: label, label: `step ${label}` }));
  const work = (live: boolean): Turn => ({
    id: "t",
    items: [
      {
        type: "work",
        id: "w",
        live,
        phases: [
          { id: "p1", kind: "edit", title: "Edited 2 files", steps: [{ id: "x", label: "src/x.ts" }] },
          { id: "p2", kind: "run", title: "Running tests", steps: [...steps, { id: "bad", label: "npm test", failed: true }] },
        ],
      },
    ],
  });

  it("keeps phases closed until pressed when tool calls are collapsed (the default)", async () => {
    const user = userEvent.setup();
    show(work(true));
    const phase = screen.getByText("Edited 2 files").closest("button")!;
    expect(phase).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("src/x.ts").closest("[aria-hidden]")).toHaveAttribute("aria-hidden", "true");
    await user.click(phase);
    expect(phase).toHaveAttribute("aria-expanded", "true");
  });

  it("opens the live phase on its latest four steps when tool calls are not collapsed", () => {
    localStorage.setItem(TOOL_CALLS_KEY, "false");
    show(work(true));
    const running = screen.getByText("Running tests").closest('[data-testid="work-phase"]') as HTMLElement;
    expect(within(running).getAllByText(/^step |^npm test$/)).toHaveLength(4);
    expect(within(running).getByText("npm test")).toHaveClass("text-deletions");
    expect(within(running).queryByText("step a")).toBeNull();
  });

  it("lists every step once the reader opens the phase", async () => {
    const user = userEvent.setup();
    localStorage.setItem(TOOL_CALLS_KEY, "false");
    show(work(true));
    // Open on its own (live): pressing it closes it, pressing again opens it with every step.
    await user.click(screen.getByText("Running tests").closest("button")!);
    await user.click(screen.getByText("Running tests").closest("button")!);
    expect(screen.getByText("step a")).toBeInTheDocument();
  });

  it("shows the phase kind icons and follows a change of the setting", () => {
    show(work(false));
    const kinds = screen.getAllByTestId("work-phase").map((node) => node.getAttribute("data-kind"));
    expect(kinds).toEqual(["edit", "run"]);
    act(() => setCollapsedToolCalls(false));
    expect(localStorage.getItem(TOOL_CALLS_KEY)).toBe("false");
  });
});

describe("footer (F-38)", () => {
  const finished: Turn = {
    id: "t",
    footer: true,
    copyText: "The answer",
    completedAt: new Date(2026, 0, 2, 9, 41).getTime(),
    durationMs: 10_000,
    items: [{ type: "text", id: "a", text: "The answer" }],
    metrics: { inputTokens: 1500, outputTokens: 400, cacheHitPercent: 60 },
  } as Turn;

  it("copies the reply inside the click and confirms for two seconds", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    // After setup: user-event installs its own clipboard stub.
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    show(finished);
    await user.click(screen.getByRole("button", { name: "Copy response" }));
    expect(writeText).toHaveBeenCalledWith("The answer");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(screen.getByText(/9:41/)).toBeInTheDocument();
  });

  it("copies the reply without control and bidi characters", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    show({ ...finished, copyText: "run\u001b[201~ this\u202e now" });
    await user.click(screen.getByRole("button", { name: "Copy response" }));
    expect(writeText).toHaveBeenCalledWith("run[201~ this now");
  });

  it("falls back to selectable text when the clipboard refuses", async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) }, configurable: true });
    show(finished);
    await user.click(screen.getByRole("button", { name: "Copy response" }));
    const dialog = await screen.findByTestId("copy-fallback-dialog");
    expect(within(dialog).getByRole("textbox", { name: "Response text" })).toHaveValue("The answer");
  });

  it("falls back when there is no clipboard at all", async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    show(finished);
    await user.click(screen.getByRole("button", { name: "Copy response" }));
    expect(await screen.findByTestId("copy-fallback-dialog")).toBeInTheDocument();
  });

  it("opens the metrics of the turn", async () => {
    const user = userEvent.setup();
    show(finished);
    await user.click(screen.getByRole("button", { name: "Turn metrics" }));
    const dialog = await screen.findByTestId("turn-metrics-dialog");
    expect(within(dialog).getByText("Cache hit")).toBeInTheDocument();
    expect(within(dialog).getByText("1,500")).toBeInTheDocument();
    expect(within(dialog).getByText(/^Finished at /)).toBeInTheDocument();
  });

  it("has no footer on a turn without one, and no metrics button without metrics", () => {
    const { container } = show({ id: "t", items: [{ type: "text", id: "a", text: "x" }] });
    expect(container.querySelector("button")).toBeNull();
    show({ id: "t2", footer: true, items: [], completedAt: 1 });
    expect(screen.queryByRole("button", { name: "Turn metrics" })).toBeNull();
  });
});

describe("notices (F-41)", () => {
  it("shows a plain notice as text and an error notice as a row that reports itself", async () => {
    const onError = vi.fn();
    const user = userEvent.setup();
    show(
      {
        id: "t",
        items: [
          { type: "notice", id: "n1", text: "Waiting for approval on desktop: run tests" },
          { type: "notice", id: "n2", text: "model overloaded", error: true },
        ],
      },
      { onError },
    );
    expect(screen.getByText("Waiting for approval on desktop: run tests")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Couldn't continue. View details" }));
    expect(onError).toHaveBeenCalledWith(expect.objectContaining({ id: "n2", text: "model overloaded" }));
  });

  it("titles the removed-account error specially", () => {
    show({ id: "t", items: [{ type: "notice", id: "n", text: "removed provider account X", error: true }] });
    expect(screen.getByRole("button", { name: "Account unavailable. View details" })).toBeInTheDocument();
  });
});
