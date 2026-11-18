// W-08: the composer on its own (F-47, F-48): Enter and Shift+Enter, Send and Stop, attachments, read-only,
// offline, a refused send, the error takeover bar.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, DESKTOP_LAYOUT_QUERY, OFFLINE_PLACEHOLDER, READ_ONLY_REASON, type ComposerProps } from "./Composer";
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

  it("a draft mid-turn still sends with the follow-up placeholder; an empty draft shows Stop", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn(() => Promise.resolve());
    const { onSubmit, field } = setup({ busy: true, onStop });
    expect(field).toHaveAttribute("placeholder", "Send a follow-up...");
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
    await user.type(field, "also this");
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith("also this", []));
  });

  it("keeps its layout when Stop takes focus, so the press is not moved from under the pointer", () => {
    const { field } = setup({ busy: true, onStop: () => Promise.resolve() });
    const shape = () => field.closest("[data-shape]");
    expect(shape()).toHaveAttribute("data-shape", "pill");
    // A pointer press focuses the button on pointerdown; a layout change there would swallow the click.
    act(() => screen.getByRole("button", { name: "Stop" }).focus());
    expect(shape()).toHaveAttribute("data-shape", "pill");
    act(() => field.focus());
    expect(shape()).toHaveAttribute("data-shape", "card");
  });

  it("shows why Stop failed", async () => {
    const user = userEvent.setup();
    setup({ busy: true, onStop: () => Promise.reject(new Error("timeout")) });
    await user.click(screen.getByRole("button", { name: "Stop" }));
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

  it("an error notice takes the field's place until it is tapped away", async () => {
    const user = userEvent.setup();
    const dismiss = vi.fn();
    setup({ notice: { text: "Rate limited", dismiss } });
    expect(screen.queryByRole("textbox")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Couldn't continue: Rate limited" }));
    expect(dismiss).toHaveBeenCalled();
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
