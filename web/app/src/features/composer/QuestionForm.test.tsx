import type { QuestionPrompt } from "@monoptah/protocol";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QuestionForm } from "./QuestionForm";

function prompt(over: Partial<QuestionPrompt> = {}, multiSelect = false): QuestionPrompt {
  return {
    requestId: 7,
    questions: [
      {
        id: "colour",
        prompt: "Pick a colour",
        multiSelect,
        allowCustom: false,
        options: [
          { id: "red", label: "Red" },
          { id: "green", label: "Green" },
          { id: "blue", label: "Blue" },
        ],
      },
    ],
    ...over,
  };
}

describe("QuestionForm", () => {
  it("moves with arrow keys, selects with Enter and sends an answered reply", async () => {
    const user = userEvent.setup();
    const onReply = vi.fn();
    render(<QuestionForm prompt={prompt()} onReply={onReply} />);
    const options = screen.getAllByRole("radio");
    expect(options[0]).toHaveAttribute("tabindex", "0");
    expect(options[1]).toHaveAttribute("tabindex", "-1");
    act(() => options[0]!.focus());
    await user.keyboard("{ArrowDown}");
    expect(options[1]).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(options[1]).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(onReply).toHaveBeenCalledWith(7, { kind: "answered", answers: { colour: ["green"] } });
  });

  it("selects by number shortcut and wraps with Home and End", async () => {
    const user = userEvent.setup();
    render(<QuestionForm prompt={prompt()} onReply={vi.fn()} />);
    const options = screen.getAllByRole("radio");
    act(() => options[0]!.focus());
    await user.keyboard("3");
    expect(options[2]).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Home}");
    expect(options[0]).toHaveFocus();
    await user.keyboard("{End}");
    expect(options[2]).toHaveFocus();
  });

  it("uses checkboxes for multi-select and keeps several choices", async () => {
    const user = userEvent.setup();
    const onReply = vi.fn();
    render(<QuestionForm prompt={prompt({}, true)} onReply={onReply} />);
    await user.click(screen.getByRole("checkbox", { name: "Red" }));
    await user.click(screen.getByRole("checkbox", { name: "Blue" }));
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(onReply).toHaveBeenCalledWith(7, { kind: "answered", answers: { colour: ["red", "blue"] } });
  });

  it("keeps Send disabled until the question is complete and Skip sends skipped", async () => {
    const user = userEvent.setup();
    const onReply = vi.fn();
    render(<QuestionForm prompt={prompt()} onReply={onReply} />);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Skip" }));
    expect(onReply).toHaveBeenCalledWith(7, { kind: "skipped" });
  });

  it("requires text for the Other option and sends it as custom", async () => {
    const user = userEvent.setup();
    const onReply = vi.fn();
    const p = prompt();
    p.questions[0]!.allowCustom = true;
    render(<QuestionForm prompt={p} onReply={onReply} />);
    await user.click(screen.getByRole("radio", { name: "Other" }));
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Your answer" }), "Purple");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(onReply).toHaveBeenCalledWith(7, {
      kind: "answered",
      answers: { colour: ["__custom__"] },
      custom: { colour: "Purple" },
    });
  });

  it("steps through several questions with Back and Continue", async () => {
    const user = userEvent.setup();
    const onReply = vi.fn();
    const p = prompt();
    p.questions.push({ id: "size", prompt: "Pick a size", multiSelect: false, allowCustom: false, options: [{ id: "s", label: "Small" }] });
    render(<QuestionForm prompt={p} onReply={onReply} />);
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "Red" }));
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("Pick a size")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("radio", { name: "Red" })).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await user.click(screen.getByRole("radio", { name: "Small" }));
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(onReply).toHaveBeenCalledWith(7, { kind: "answered", answers: { colour: ["red"], size: ["s"] } });
  });

  it("shows an error and stays open when the reply fails", async () => {
    const user = userEvent.setup();
    render(<QuestionForm prompt={prompt()} onReply={vi.fn().mockRejectedValue(new Error("Link is down"))} />);
    await user.click(screen.getByRole("radio", { name: "Red" }));
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Link is down");
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });

  it("is read only with an explanation when a reason is given", async () => {
    render(<QuestionForm prompt={prompt()} onReply={vi.fn()} disabledReason="This device is read only." />);
    expect(screen.getByText("This device is read only.")).toBeInTheDocument();
    for (const option of screen.getAllByRole("radio")) expect(option).toBeDisabled();
    expect(screen.getByRole("button", { name: "Skip" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});

describe("QuestionForm countdown and keep open", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
  });
  afterEach(() => vi.useRealTimers());

  it("says optional beyond 60 s, then counts down each second", () => {
    render(<QuestionForm prompt={prompt({ autoResolveAt: 1_000_000 + 90_000 })} onReply={vi.fn()} />);
    expect(screen.getByText("Optional question")).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(31_000);
    });
    expect(screen.getByText("Continues without an answer in 59s")).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(screen.getByText("Continues without an answer in 58s")).toBeInTheDocument();
  });

  it("calls keepQuestionOpen once on the first touch of a timed question", () => {
    const onKeepOpen = vi.fn().mockResolvedValue(undefined);
    render(<QuestionForm prompt={prompt({ autoResolveAt: 1_000_000 + 30_000 })} onReply={vi.fn()} onKeepOpen={onKeepOpen} />);
    fireEvent.pointerDown(screen.getByRole("radio", { name: "Red" }));
    fireEvent.keyDown(screen.getByRole("radio", { name: "Red" }), { key: "ArrowDown" });
    expect(onKeepOpen).toHaveBeenCalledTimes(1);
    expect(onKeepOpen).toHaveBeenCalledWith(7);
  });

  it("never calls keepQuestionOpen for an untimed question", () => {
    const onKeepOpen = vi.fn();
    render(<QuestionForm prompt={prompt()} onReply={vi.fn()} onKeepOpen={onKeepOpen} />);
    fireEvent.pointerDown(screen.getByRole("radio", { name: "Red" }));
    expect(onKeepOpen).not.toHaveBeenCalled();
  });

  it("retries the keep-open call on a later touch after a failure", async () => {
    const onKeepOpen = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
    render(<QuestionForm prompt={prompt({ autoResolveAt: 1_000_000 + 30_000 })} onReply={vi.fn()} onKeepOpen={onKeepOpen} />);
    const red = screen.getByRole("radio", { name: "Red" });
    fireEvent.pointerDown(red);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.pointerDown(red);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(onKeepOpen).toHaveBeenCalledTimes(2);
    fireEvent.pointerDown(red);
    expect(onKeepOpen).toHaveBeenCalledTimes(2);
  });
});
