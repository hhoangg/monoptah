// The menu in the composer with a stand-in controller: the phone sheet, the tap, the closed states.
import { render, screen, waitFor, within } from "@testing-library/react";
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
});
