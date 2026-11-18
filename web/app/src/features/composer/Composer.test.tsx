// W-08: the composer on its own (F-47, F-48): Enter and Shift+Enter, Send and Stop, attachments, read-only,
// offline, a refused send, the error takeover bar.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, DESKTOP_LAYOUT_QUERY, OFFLINE_PLACEHOLDER, READ_ONLY_REASON, STOP_RETRY_MS, UNKNOWN_ERROR_SENTENCE, type ComposerProps } from "./Composer";
import { SendCancelled } from "./useSendGuard";

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
  let n = 0;
  URL.createObjectURL = vi.fn(() => `blob:fake-${n++}`);
  URL.revokeObjectURL = vi.fn();
  window.scrollTo = vi.fn() as never;
});
afterEach(() => vi.restoreAllMocks());

/** Stop is armed STOP_ARM_DELAY_MS after it appears: wait for that, then return the button. */
async function armedStop() {
  const stop = screen.getByRole("button", { name: "Stop" });
  await waitFor(() => expect(stop).not.toHaveAttribute("aria-disabled"));
  return stop;
}

function setup(props: Partial<ComposerProps> = {}) {
  const onSubmit = vi.fn<ComposerProps["onSubmit"]>(() => Promise.resolve());
  render(<Composer onSubmit={onSubmit} {...props} />);
  return { onSubmit, field: screen.queryByRole("textbox", { name: "Message" }) as HTMLTextAreaElement };
}

describe("send", () => {
  it("is disabled until there is a draft, then sends the trimmed text and clears", async () => {
    const user = userEvent.setup();
    const { onSubmit, field } = setup();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.type(field, "  hello  ");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(field.value).toBe(""));
    expect(onSubmit).toHaveBeenCalledWith("hello", []);
  });

  it("sends on Enter on a desktop layout and keeps Shift+Enter for a newline", async () => {
    const user = userEvent.setup();
    const { onSubmit, field } = setup();
    await user.type(field, "line one{Shift>}{Enter}{/Shift}line two");
    expect(field.value).toBe("line one\nline two");
    expect(onSubmit).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith("line one\nline two", []));
  });

  it("does not send on a plain Enter on a touch layout; Ctrl+Enter still does", async () => {
    setMedia(false);
    const user = userEvent.setup();
    const { onSubmit, field } = setup();
    await user.type(field, "hi{Enter}there");
    expect(field.value).toBe("hi\nthere");
    expect(onSubmit).not.toHaveBeenCalled();
    await user.keyboard("{Control>}{Enter}{/Control}");
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
  });

  it("ignores Enter while an IME composition is open", () => {
    const { onSubmit, field } = setup();
    fireEvent.change(field, { target: { value: "konnichiwa" } });
    fireEvent.keyDown(field, { key: "Enter", isComposing: true, keyCode: 229 });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("keeps the draft and says why when the send is refused, and stays silent when the user cancels", async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn<ComposerProps["onSubmit"]>().mockRejectedValueOnce(new Error("The instance is starting")).mockRejectedValueOnce(new SendCancelled("no"));
    const { field } = setup({ onSubmit });
    await user.type(field, "go");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The instance is starting");
    expect(field.value).toBe("go");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(field.value).toBe("go");
  });

  it("a draft mid-turn still sends with the follow-up placeholder; Stop stays next to Send", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    const { onSubmit, field } = setup({ busy: true, onStop });
    expect(field).toHaveAttribute("placeholder", "Send a follow-up...");
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    await user.click(await armedStop());
    expect(onStop).toHaveBeenCalledTimes(1);
    await user.type(field, "also this");
    expect(screen.getByRole("button", { name: "Stopping..." })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith("also this", []));
  });

  it("ignores a press on Stop until it has been there for the arming delay", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    setup({ busy: true, onStop });
    const stop = screen.getByRole("button", { name: "Stop" });
    expect(stop).toHaveAttribute("aria-disabled", "true");
    await user.click(stop);
    expect(onStop).not.toHaveBeenCalled();
    await armedStop();
    await user.click(stop);
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("says Stopping... with aria-busy until the turn ends, not only until the cancel resolves", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    const onSubmit = vi.fn<ComposerProps["onSubmit"]>(() => Promise.resolve());
    const { rerender } = render(<Composer onSubmit={onSubmit} busy onStop={onStop} />);
    await user.click(await armedStop());
    const stop = screen.getByRole("button", { name: "Stopping..." });
    await waitFor(() => expect(stop).toHaveAttribute("aria-busy", "true"));
    // Visible text inside the button (touch has no tooltip) and one polite live region outside it.
    expect(stop).toHaveTextContent("Stopping...");
    expect(screen.getByRole("status")).toHaveTextContent("Stopping...");
    expect(stop).not.toContainElement(screen.getByRole("status"));
    await user.click(stop);
    expect(onStop).toHaveBeenCalledTimes(1);
    rerender(<Composer onSubmit={onSubmit} busy={false} onStop={onStop} />);
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it("re-arms Stop when it changes slot, so a double tap on Send never hits an armed Stop", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    const { field } = setup({ busy: true, onStop });
    await armedStop();
    await user.type(field, "x");
    // The draft moved Stop to the slot beside Send: not armed until the delay passes again.
    expect(screen.getByRole("button", { name: "Stop" })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).not.toHaveBeenCalled();
    await armedStop();
    await user.clear(field);
    expect(screen.getByRole("button", { name: "Stop" })).toHaveAttribute("aria-disabled", "true");
    await armedStop();
  });

  it("lets Stop be pressed again when the turn still runs after the cancel request resolved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const onStop = vi.fn(() => Promise.resolve());
      setup({ busy: true, onStop });
      await user.click(await armedStop());
      const stop = screen.getByRole("button", { name: "Stopping..." });
      await waitFor(() => expect(stop).toHaveAttribute("aria-busy", "true"));
      expect(stop).toHaveAttribute("aria-disabled", "true");
      await user.click(stop);
      expect(onStop).toHaveBeenCalledTimes(1);
      await act(async () => {
        vi.advanceTimersByTime(STOP_RETRY_MS + 50);
      });
      expect(stop).not.toHaveAttribute("aria-disabled");
      expect(stop).toHaveTextContent("Stopping...");
      await user.click(stop);
      expect(onStop).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps its layout when Stop takes pointer focus, so the press is not moved from under the pointer", () => {
    // jsdom matches `:focus-visible` for every focus; a browser does not for a pointer press, so say it does not.
    const matches = Element.prototype.matches;
    const spy = vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, selector: string) {
      return selector === ":focus-visible" ? false : matches.call(this, selector);
    });
    try {
      const { field } = setup({ busy: true, onStop: () => Promise.resolve() });
      const shape = () => field.closest("[data-shape]");
      expect(shape()).toHaveAttribute("data-shape", "pill");
      act(() => screen.getByRole("button", { name: "Stop" }).focus());
      expect(shape()).toHaveAttribute("data-shape", "pill");
      act(() => field.focus());
      expect(shape()).toHaveAttribute("data-shape", "card");
    } finally {
      spy.mockRestore();
    }
  });

  it("a pointer press on Stop does not morph the pill and does not steal focus from the field", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    const { field } = setup({ busy: true, onStop });
    const shape = () => field.closest("[data-shape]");
    const stop = await armedStop();
    await user.pointer({ keys: "[MouseLeft>]", target: stop });
    expect(shape()).toHaveAttribute("data-shape", "pill");
    expect(stop).not.toHaveFocus();
    await user.pointer({ keys: "[/MouseLeft]", target: stop });
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("keeps the card when Stop is pressed while the empty field has focus", async () => {
    const user = userEvent.setup();
    const { field } = setup({ busy: true, onStop: () => Promise.resolve() });
    const shape = () => field.closest("[data-shape]");
    act(() => field.focus());
    expect(shape()).toHaveAttribute("data-shape", "card");
    await user.pointer({ keys: "[MouseLeft>]", target: screen.getByRole("button", { name: "Stop" }) });
    expect(field).toHaveFocus();
    expect(shape()).toHaveAttribute("data-shape", "card");
  });

  it("shows why Stop failed", async () => {
    const user = userEvent.setup();
    setup({ busy: true, onStop: () => Promise.reject(new Error("timeout")) });
    await user.click(await armedStop());
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not stop: timeout");
  });
});

describe("disabled states", () => {
  it("a read-only device cannot write or send and sees the reason", async () => {
    const { field } = setup({ readOnly: true });
    expect(field).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add photos or files" })).toBeDisabled();
    expect(screen.getByText(READ_ONLY_REASON)).toBeInTheDocument();
    expect(field.getAttribute("aria-describedby")).toBe(screen.getByText(READ_ONLY_REASON).id);
  });

  it("offline keeps writing open but holds Send and Stop", async () => {
    const user = userEvent.setup();
    const { field } = setup({ offline: true, busy: true, onStop: () => Promise.resolve() });
    expect(field).toHaveAttribute("placeholder", OFFLINE_PLACEHOLDER);
    expect(screen.getByRole("button", { name: "Stop" })).toBeDisabled();
    await user.type(field, "draft");
    expect(field.value).toBe("draft");
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("an error notice is a compact row above the field: the field and toolbar stay usable", async () => {
    const user = userEvent.setup();
    const dismiss = vi.fn();
    setup({ notice: { text: "Rate limited", dismiss }, toolbar: <button type="button">Model</button> });
    expect(screen.getByRole("textbox", { name: "Message" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Model" })).toBeEnabled();
    const row = screen.getByTestId("composer-notice");
    expect(row).toHaveTextContent("Couldn't continue");
    expect(row).toHaveTextContent("Rate limited");
    await user.click(screen.getByRole("button", { name: "Details" }));
    expect(await screen.findByTestId("session-error-dialog")).toHaveTextContent("Rate limited");
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(dismiss).toHaveBeenCalled();
  });

  it("says an unknown or empty error code in a plain sentence", () => {
    setup({ notice: { text: "unknown", dismiss: () => {} } });
    expect(screen.getByTestId("composer-notice")).toHaveTextContent(UNKNOWN_ERROR_SENTENCE);
    expect(screen.getByTestId("composer-notice")).not.toHaveTextContent(/^unknown$/);
  });

  it("raises no notice for a turn the user stopped (the desktop sends \"Stopped\")", () => {
    setup({ notice: { text: "Stopped", dismiss: () => {} } });
    expect(screen.queryByTestId("composer-notice")).toBeNull();
  });

  it("still shows a notice that only mentions stopping or cancelling", () => {
    setup({ notice: { text: "Stopped Docker sandbox box-1 after 10 min idle; send a message to resume.", dismiss: () => {} } });
    expect(screen.getByTestId("composer-notice")).toHaveTextContent("Stopped Docker sandbox box-1");
  });

  it("still shows an Interrupted or Cancelled notice: only the desktop's own wording is hidden", () => {
    setup({ notice: { text: "Connection cancelled", dismiss: () => {} } });
    expect(screen.getByTestId("composer-notice")).toHaveTextContent("Connection cancelled");
  });

  it("a read-only device can ask for control, and then sees that it was requested", async () => {
    const user = userEvent.setup();
    const request = vi.fn();
    const { rerender } = render(<Composer onSubmit={vi.fn()} readOnly requestControl={{ state: "idle", request }} />);
    await user.click(screen.getByRole("button", { name: "Request control" }));
    expect(request).toHaveBeenCalledTimes(1);
    rerender(<Composer onSubmit={vi.fn()} readOnly requestControl={{ state: "requested", request }} />);
    expect(screen.getByText("Requested, confirm on the desktop.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Request control" })).toBeNull();
    // The user is never stuck: asking again re-sends the request.
    await user.click(screen.getByRole("button", { name: "Ask again" }));
    expect(request).toHaveBeenCalledTimes(2);
  });
});

describe("attachments (F-48)", () => {
  const file = (name: string, size = 10, type = "text/plain") => new File([new Uint8Array(size)], name, { type });

  it("adds picked files as chips, sends them and revokes nothing before the send", async () => {
    const user = userEvent.setup();
    const { onSubmit } = setup();
    await user.upload(screen.getByTestId("composer-file-input"), [file("a.txt"), file("b.txt")]);
    const list = await screen.findByRole("list", { name: "Attachments" });
    expect(list.querySelectorAll("[data-attachment-chip]")).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const [text, sent] = onSubmit.mock.calls[0]!;
    expect(text).toBe("");
    expect(sent.map((item) => item.name)).toEqual(["a.txt", "b.txt"]);
    expect(sent[0]!.mimeType).toBe("text/plain");
    await waitFor(() => expect(screen.queryByRole("list", { name: "Attachments" })).toBeNull());
  });

  it("removes a chip", async () => {
    const user = userEvent.setup();
    setup();
    await user.upload(screen.getByTestId("composer-file-input"), [file("a.txt")]);
    const list = await screen.findByRole("list", { name: "Attachments" });
    await user.click(list.querySelector("button")!);
    await waitFor(() => expect(screen.queryByRole("list", { name: "Attachments" })).toBeNull());
  });

  it("keeps ten files and says so for the eleventh", async () => {
    const user = userEvent.setup();
    setup();
    await user.upload(screen.getByTestId("composer-file-input"), Array.from({ length: 10 }, (_, i) => file(`f${i}.txt`)));
    await waitFor(() => expect(screen.getByRole("list", { name: "Attachments" }).children).toHaveLength(10));
    await user.upload(screen.getByTestId("composer-file-input"), [file("extra.txt")]);
    expect(await screen.findByRole("alert")).toHaveTextContent("up to 10 files");
    expect(screen.getByRole("list", { name: "Attachments" }).children).toHaveLength(10);
  });

  it("refuses a set that is over 8 MiB of base64", async () => {
    const user = userEvent.setup();
    setup();
    // Two 4 MiB files are 2 x 5.6 MB of base64: together over the message limit.
    await user.upload(screen.getByTestId("composer-file-input"), [file("big1.bin", 4 * 1024 * 1024, "application/octet-stream")]);
    await screen.findByRole("list", { name: "Attachments" });
    await user.upload(screen.getByTestId("composer-file-input"), [file("big2.bin", 4 * 1024 * 1024, "application/octet-stream")]);
    expect(await screen.findByRole("alert")).toHaveTextContent("8 MiB per message");
    expect(screen.getByRole("list", { name: "Attachments" }).children).toHaveLength(1);
  });

  it("takes files from a paste", async () => {
    setup();
    const field = screen.getByRole("textbox", { name: "Message" });
    fireEvent.paste(field, { clipboardData: { files: [file("shot.png", 10, "image/png")], items: [] } });
    expect(await screen.findByRole("list", { name: "Attachments" })).toBeInTheDocument();
  });
});

describe("dictation (F-49)", () => {
  class FakeRecognition {
    static last: FakeRecognition | null = null;
    lang = "";
    continuous = false;
    interimResults = false;
    onresult: ((event: unknown) => void) | null = null;
    onerror: unknown = null;
    onend: (() => void) | null = null;
    constructor() {
      FakeRecognition.last = this;
    }
    start() {}
    stop() {
      this.onend?.();
    }
    abort() {}
  }
  const phrase = (text: string) => ({ resultIndex: 0, results: { length: 1, 0: { isFinal: true, length: 1, 0: { transcript: text } } } });
  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).webkitSpeechRecognition;
  });

  it("has no microphone where the Web Speech API is missing", () => {
    setup();
    expect(screen.queryByRole("button", { name: "Dictation" })).toBeNull();
  });

  it("adds each finished phrase to the draft after what is typed, without sending", async () => {
    (window as unknown as Record<string, unknown>).webkitSpeechRecognition = FakeRecognition;
    const user = userEvent.setup();
    const { onSubmit, field } = setup();
    await user.type(field, "Fix");
    await user.click(screen.getByRole("button", { name: "Dictation" }));
    act(() => FakeRecognition.last?.onresult?.(phrase("the build")));
    expect(field.value).toBe("Fix the build");
    act(() => FakeRecognition.last?.onresult?.(phrase("please")));
    expect(field.value).toBe("Fix the build please");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("is off for a read-only device", () => {
    (window as unknown as Record<string, unknown>).webkitSpeechRecognition = FakeRecognition;
    setup({ readOnly: true });
    expect(screen.getByRole("button", { name: "Dictation" })).toBeDisabled();
  });
});
