// The virtualized transcript (F-32, F-33) in jsdom, with the layout stubbed by layoutStub.ts.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Turn } from "@monoptah/protocol";
import type { OptimisticTurn } from "@/data";
import { ROW, VIEW, scrollTops, stubTranscriptLayout } from "./layoutStub";
import { Transcript } from "./Transcript";

beforeAll(() => {
  stubTranscriptLayout();
  window.matchMedia ??= ((query: string) => ({ matches: false, media: query, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
});

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => vi.restoreAllMocks());

const turn = (n: number): Turn => ({ id: `t${n}`, items: [{ type: "text", id: `a${n}`, text: `Reply number ${n}` }] });
const turns = (from: number, to: number): Turn[] => Array.from({ length: to - from }, (_, i) => turn(from + i));

const scroller = () => screen.getByTestId("transcript-scroller");
const rowStart = (turnId: string) => {
  const row = document.querySelector(`[data-turn-id="${turnId}"]`)!.closest<HTMLElement>("[data-index]")!;
  return parseFloat(/translateY\(([-\d.]+)px\)/.exec(row.style.transform)![1]!);
};
const scrollTo = (top: number) => {
  scrollTops.set(scroller(), top);
  fireEvent.scroll(scroller());
};

describe("windowing and the end (F-32)", () => {
  it("renders only the rows near the viewport and opens at the end", async () => {
    render(<Transcript turns={turns(0, 200)} />);
    expect(screen.getAllByTestId("transcript-turn").length).toBeLessThan(30);
    expect(await screen.findByText("Reply number 199")).toBeInTheDocument();
    // Rows not yet measured count by estimate, so compare with the scroller's own height.
    await waitFor(() => expect(scroller().scrollTop).toBe(scroller().scrollHeight - VIEW));
    expect(screen.queryByText("Reply number 0")).toBeNull();
  });

  it("follows a reply that grows while the reader is at the end", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    rerender(<Transcript turns={turns(0, 21)} />);
    expect(await screen.findByText("Reply number 20")).toBeInTheDocument();
    await waitFor(() => expect(scroller().scrollTop).toBe(scroller().scrollHeight - VIEW));
  });

  it("does not drag a reader who scrolled up to the end", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    scrollTo(500);
    rerender(<Transcript turns={turns(0, 21)} />);
    expect(scroller().scrollTop).toBe(500);
  });

  it("shows an empty and a loading state", () => {
    const { rerender } = render(<Transcript turns={[]} />);
    expect(screen.getByText("No messages yet.")).toBeInTheDocument();
    rerender(<Transcript turns={[]} loading />);
    expect(screen.queryByText("No messages yet.")).toBeNull();
  });
});

describe("anchoring the latest user message (F-32)", () => {
  const userTurn = (id: string): Turn => ({ id, items: [{ type: "user", id: `${id}:u`, text: `Question ${id}` }] });
  const pendingTurn = (id = "pending:1"): OptimisticTurn => ({ id, text: "Question sent here", at: 1, attachments: 0, status: "sending", baseline: 0 });
  const REST = 8;
  const end = () => scroller().scrollHeight - VIEW;

  it("a message sent here rests near the top with blank room below it, and the reply grows into that room first", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    rerender(<Transcript turns={turns(0, 20)} optimistic={[pendingTurn()]} />);
    await screen.findByText("Question sent here");
    const anchored = rowStart("pending:1") - REST;
    await waitFor(() => expect(scroller().scrollTop).toBe(anchored));
    // The room below is blank: the scroll height reaches past the content, and the anchor is exactly its end.
    expect(end()).toBe(anchored);
    expect(scroller().scrollHeight).toBeGreaterThan(rowStart("pending:1") + ROW + 16);

    // The desktop confirms the message and a reply of one row fills part of the room: the message does not move.
    const confirmed = [...turns(0, 20), userTurn("u1")];
    rerender(<Transcript turns={confirmed} />);
    await screen.findByText("Question u1");
    expect(scroller().scrollTop).toBe(anchored);
    rerender(<Transcript turns={[...confirmed, turn(21)]} />);
    await screen.findByText("Reply number 21");
    expect(scroller().scrollTop).toBe(anchored);

    // Once the reply is taller than the room the list follows the end as usual.
    rerender(<Transcript turns={[...confirmed, turn(21), turn(22), turn(23), turn(24), turn(25)]} />);
    await screen.findByText("Reply number 25");
    await waitFor(() => expect(scroller().scrollTop).toBe(end()));
    expect(scroller().scrollTop).toBeGreaterThan(anchored);
  });

  it("the desktop confirming the pending message does not move the anchor", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    rerender(<Transcript turns={turns(0, 20)} optimistic={[pendingTurn()]} />);
    await screen.findByText("Question sent here");
    await waitFor(() => expect(scroller().scrollTop).toBe(rowStart("pending:1") - REST));
    const anchored = scroller().scrollTop;
    rerender(<Transcript turns={[...turns(0, 20), userTurn("u1")]} />);
    await screen.findByText("Question u1");
    expect(scroller().scrollTop).toBe(anchored);
    expect(rowStart("u1") - REST).toBe(anchored);
  });

  it("a message the reader sends goes to the anchor even from far up the history", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    scrollTo(500);
    rerender(<Transcript turns={turns(0, 20)} optimistic={[pendingTurn()]} />);
    await screen.findByText("Question sent here");
    // Sending is the user's own action: it goes to the anchor (the sent message is what they want to see).
    await waitFor(() => expect(scroller().scrollTop).toBe(rowStart("pending:1") - REST));
  });

  it("opening an idle session lands at the end; opening one that is working anchors its latest user message", async () => {
    const history = [...turns(0, 18), userTurn("u1"), turn(19)];
    const idle = render(<Transcript turns={history} />);
    await screen.findByText("Question u1");
    await waitFor(() => expect(scroller().scrollTop).toBe(end()));
    expect(end()).toBe(scroller().scrollHeight - VIEW);
    expect(rowStart("u1") - REST).not.toBe(scroller().scrollTop);
    idle.unmount();

    render(<Transcript turns={history} busy />);
    await screen.findByText("Question u1");
    await waitFor(() => expect(scroller().scrollTop).toBe(rowStart("u1") - REST));
    expect(end()).toBe(scroller().scrollTop);
  });
});

describe("jump to bottom (F-33)", () => {
  it("appears when the reader is more than 160 px from the end and takes them back", async () => {
    const user = userEvent.setup();
    render(<Transcript turns={turns(0, 50)} />);
    await screen.findByText("Reply number 49");
    const end = scroller().scrollTop;
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    scrollTo(end - 150);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    scrollTo(end - 170);
    const jump = await screen.findByRole("button", { name: "Jump to latest" });
    await user.click(jump);
    expect(scroller().scrollTop).toBe(end);
    scrollTo(end);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  });
});

describe("jump to bottom while the list is still measuring", () => {
  const jumpButton = () => screen.queryByRole("button", { name: "Jump to latest" });
  async function awayFromEnd() {
    const view = render(<Transcript turns={turns(0, 50)} />);
    await screen.findByText("Reply number 49");
    const end = scroller().scrollTop;
    scrollTo(end - 1000);
    const jump = await screen.findByRole("button", { name: "Jump to latest" });
    // A smooth scroll that the test steps by hand.
    const smooth = vi.fn();
    scroller().scrollTo = smooth as never;
    return { ...view, end, jump, smooth };
  }

  it("keeps following through the smooth scroll and lands at the real end after heights change", async () => {
    const user = userEvent.setup();
    const { rerender, end, jump, smooth } = await awayFromEnd();
    await user.click(jump);
    expect(smooth).toHaveBeenCalledWith({ top: end, behavior: "smooth" });
    expect(jumpButton()).toBeNull();
    // An intermediate scroll event far from the end must not turn following off or bring the button back.
    scrollTo(end - 600);
    expect(jumpButton()).toBeNull();
    // The list grows while the scroll runs: the end moves and the view lands at the new one.
    rerender(<Transcript turns={turns(0, 53)} />);
    await screen.findByText("Reply number 52");
    await waitFor(() => expect(scroller().scrollTop).toBe(scroller().scrollHeight - VIEW));
    scrollTo(scroller().scrollHeight - VIEW);
    expect(jumpButton()).toBeNull();
    // Later output keeps being followed.
    rerender(<Transcript turns={turns(0, 55)} />);
    await screen.findByText("Reply number 54");
    await waitFor(() => expect(scroller().scrollTop).toBe(scroller().scrollHeight - VIEW));
  });

  it("lands at the end when the smooth scroll stops short", async () => {
    const user = userEvent.setup();
    const { end, jump } = await awayFromEnd();
    await user.click(jump);
    scrollTo(end - 300);
    await waitFor(() => expect(scroller().scrollTop).toBe(end));
    expect(jumpButton()).toBeNull();
  });

  it("is cancelled by a reader gesture and leaves following off", async () => {
    const user = userEvent.setup();
    const { rerender, end, jump } = await awayFromEnd();
    await user.click(jump);
    fireEvent.wheel(scroller(), { deltaY: -120 });
    scrollTo(end - 600);
    await screen.findByRole("button", { name: "Jump to latest" });
    const before = scroller().scrollHeight;
    rerender(<Transcript turns={turns(0, 53)} />);
    await waitFor(() => expect(scroller().scrollHeight).toBeGreaterThan(before));
    expect(scroller().scrollTop).toBe(end - 600);
  });

  it("is not cancelled by a wheel that a nested scroller takes", async () => {
    const user = userEvent.setup();
    const { end, jump } = await awayFromEnd();
    await user.click(jump);
    const inner = document.querySelector<HTMLElement>('[data-testid="transcript-turn"]')!;
    inner.style.overflowY = "auto";
    Object.defineProperty(inner, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(inner, "clientHeight", { configurable: true, value: 100 });
    fireEvent.wheel(inner, { deltaY: 120 });
    scrollTo(end - 600);
    expect(jumpButton()).toBeNull();
  });
});

describe("older turns (F-32, N-16)", () => {
  it("asks for the page when the reader reaches the top and keeps their row where it was as the page lands", async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const loadOlder = vi.fn();
    function Held() {
      const [held, setHeld] = useState(() => turns(40, 100));
      return (
        <Transcript
          turns={held}
          hasOlder={held[0]!.id !== "t0"}
          loadOlder={async () => {
            loadOlder();
            await gate;
            setHeld((previous) => [...turns(20, 40), ...previous]);
          }}
        />
      );
    }
    render(<Held />);
    await screen.findByText("Reply number 99");
    // The reveal frame must pass first: paging waits for the first scroll to the end.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 40));
    });
    expect(loadOlder).not.toHaveBeenCalled();
    scrollTo(250);
    await waitFor(() => expect(loadOlder).toHaveBeenCalledTimes(1));
    // The row at the top edge of the viewport, and where it sits below it.
    const rendered = () => [...document.querySelectorAll<HTMLElement>("[data-index]")].map((row) => ({ id: row.querySelector("[data-turn-id]")!.getAttribute("data-turn-id")!, start: rowStart(row.querySelector("[data-turn-id]")!.getAttribute("data-turn-id")!) }));
    await waitFor(() => expect(rendered().length).toBeGreaterThan(3));
    const anchor = rendered().find((row) => row.start + ROW > scroller().scrollTop)!;
    const distance = anchor.start - scroller().scrollTop;
    const topBefore = scroller().scrollTop;
    await act(async () => {
      release();
      await gate;
    });
    await waitFor(() => expect(document.querySelector('[data-turn-id="t20"]') ?? document.querySelector('[data-turn-id="t25"]') ?? document.querySelector('[data-turn-id="t30"]')).not.toBeNull());
    // Twenty rows landed above: the reader moved down with them and the same row is the same distance from the top.
    expect(scroller().scrollTop).toBeGreaterThan(topBefore + 1500);
    expect(rowStart(anchor.id) - scroller().scrollTop).toBe(distance);
    expect(loadOlder).toHaveBeenCalledTimes(1);
  });

  it("shows a loading indicator and a retry after a failed page", async () => {
    const user = userEvent.setup();
    const loadOlder = vi.fn().mockResolvedValue(undefined);
    const { rerender } = render(<Transcript turns={turns(40, 60)} hasOlder loadingOlder loadOlder={loadOlder} />);
    expect(screen.getByText("Loading earlier messages")).toBeInTheDocument();
    rerender(<Transcript turns={turns(40, 60)} hasOlder olderError="timeout" loadOlder={loadOlder} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Could not load earlier messages.");
    // The list reveals itself one animation frame after the first render, and nothing loads before that. Wait for
    // it, or a click that lands first (on a busy machine) is ignored and the test fails by timing alone.
    await waitFor(() => expect(document.querySelector(".opacity-100")).not.toBeNull());
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(loadOlder).toHaveBeenCalledTimes(1);
  });

  it("does not ask again by itself after a failed page, only the Retry button does", async () => {
    const user = userEvent.setup();
    const loadOlder = vi.fn();
    function Failing() {
      const [state, setState] = useState<{ loading: boolean; error: string | null }>({ loading: false, error: null });
      return (
        <Transcript
          turns={turns(0, 3)}
          hasOlder
          loadingOlder={state.loading}
          olderError={state.error}
          loadOlder={async () => {
            loadOlder();
            setState({ loading: true, error: null });
            await Promise.resolve();
            setState({ loading: false, error: "offline" });
          }}
        />
      );
    }
    render(<Failing />);
    await screen.findByRole("alert");
    // A short transcript sits at the top: without the guard this spins. Give it time to loop.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 150));
    });
    expect(loadOlder).toHaveBeenCalledTimes(1);
    await user.click(within(screen.getByRole("alert")).getByRole("button", { name: "Retry" }));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 100));
    });
    expect(loadOlder).toHaveBeenCalledTimes(2);
  });
});

describe("optimistic turns and errors", () => {
  const optimistic = (status: OptimisticTurn["status"], extra: Partial<OptimisticTurn> = {}): OptimisticTurn => ({
    id: "pending:1",
    text: "Run the tests",
    at: 1,
    attachments: 0,
    status,
    baseline: 0,
    ...extra,
  });

  it("shows a message on its way", async () => {
    render(<Transcript turns={turns(0, 2)} optimistic={[optimistic("sending")]} />);
    expect(await screen.findByText("Run the tests")).toBeInTheDocument();
    expect(screen.getByText("Sending...")).toBeInTheDocument();
  });

  it('shows "Not sent, retry" and sends again only when pressed', async () => {
    const onRetry = vi.fn();
    const user = userEvent.setup();
    render(<Transcript turns={turns(0, 2)} optimistic={[optimistic("not_sent", { error: "timeout" })]} onRetry={onRetry} />);
    const retry = await screen.findByRole("button", { name: "Not sent, retry" });
    expect(onRetry).not.toHaveBeenCalled();
    await user.click(retry);
    expect(onRetry).toHaveBeenCalledWith("pending:1");
  });

  it("goes to the end when the user sends", async () => {
    const { rerender } = render(<Transcript turns={turns(0, 20)} />);
    await screen.findByText("Reply number 19");
    scrollTo(300);
    rerender(<Transcript turns={turns(0, 20)} optimistic={[optimistic("sending")]} />);
    await waitFor(() => expect(scroller().scrollTop).toBe(scroller().scrollHeight - VIEW));
  });

  it("opens the details of an error notice in a dialog", async () => {
    const user = userEvent.setup();
    const errored: Turn = { id: "e", items: [{ type: "notice", id: "n", text: "model overloaded", error: true }] };
    render(<Transcript turns={[...turns(0, 2), errored]} />);
    await user.click(await screen.findByRole("button", { name: "Couldn't continue. View details" }));
    const dialog = await screen.findByTestId("session-error-dialog");
    expect(within(dialog).getByText("model overloaded")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Got it" }));
    await waitFor(() => expect(screen.queryByTestId("session-error-dialog")).toBeNull());
  });
});
