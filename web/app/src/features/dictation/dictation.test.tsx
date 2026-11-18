import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DictationButton } from "./DictationButton";
import { DICTATION_MAX_MS, type SpeechRecognitionLike } from "./useDictation";

const created: FakeRecognition[] = [];
const lastOne = () => created[created.length - 1] ?? null;

class FakeRecognition implements SpeechRecognitionLike {
  lang = "";
  continuous = false;
  interimResults = false;
  onresult: SpeechRecognitionLike["onresult"] = null;
  onerror: SpeechRecognitionLike["onerror"] = null;
  onend: SpeechRecognitionLike["onend"] = null;
  started = false;
  constructor() {
    created.push(this);
  }
  start() {
    this.started = true;
  }
  stop() {
    this.started = false;
    this.onend?.();
  }
  abort() {
    this.started = false;
  }
}

function result(text: string, isFinal: boolean) {
  return { isFinal, length: 1, 0: { transcript: text } };
}

beforeEach(() => {
  created.length = 0;
});
afterEach(() => {
  delete (window as unknown as Record<string, unknown>).webkitSpeechRecognition;
  delete (window as unknown as Record<string, unknown>).SpeechRecognition;
  vi.useRealTimers();
});

describe("DictationButton", () => {
  it("renders nothing without the Web Speech API", () => {
    const { container } = render(<DictationButton onText={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("inserts only final phrases through onText and never anything else", async () => {
    (window as unknown as Record<string, unknown>).webkitSpeechRecognition = FakeRecognition;
    const onText = vi.fn();
    const user = userEvent.setup();
    render(<DictationButton onText={onText} lang="en-US" />);
    await user.click(screen.getByRole("button", { name: "Start dictation" }));
    expect(lastOne()?.started).toBe(true);
    expect(lastOne()?.lang).toBe("en-US");
    expect(screen.getByRole("button", { name: "Stop dictation" })).toHaveAttribute("aria-pressed", "true");

    act(() => lastOne()?.onresult?.({ resultIndex: 0, results: { length: 1, 0: result("hel", false) } }));
    expect(screen.getByRole("status")).toHaveTextContent("hel");
    expect(onText).not.toHaveBeenCalled();

    act(() => lastOne()?.onresult?.({ resultIndex: 0, results: { length: 1, 0: result(" hello world ", true) } }));
    expect(onText).toHaveBeenCalledExactlyOnceWith("hello world");

    await user.click(screen.getByRole("button", { name: "Stop dictation" }));
    expect(screen.getByRole("button", { name: "Start dictation" })).toHaveAttribute("aria-pressed", "false");
  });

  it("stops after the 5 minute cap", () => {
    vi.useFakeTimers();
    (window as unknown as Record<string, unknown>).SpeechRecognition = FakeRecognition;
    render(<DictationButton onText={() => {}} />);
    act(() => screen.getByRole("button", { name: "Start dictation" }).click());
    expect(lastOne()?.started).toBe(true);
    act(() => {
      vi.advanceTimersByTime(DICTATION_MAX_MS);
    });
    expect(lastOne()?.started).toBe(false);
    expect(screen.getByRole("button", { name: "Start dictation" })).toBeInTheDocument();
  });

  it("announces a denied microphone", async () => {
    (window as unknown as Record<string, unknown>).SpeechRecognition = FakeRecognition;
    const user = userEvent.setup();
    render(<DictationButton onText={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Start dictation" }));
    act(() => lastOne()?.onerror?.({ error: "not-allowed" }));
    expect(screen.getByRole("status")).toHaveTextContent("Microphone access was denied.");
  });
});
