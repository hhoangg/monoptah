import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { applyModifiers, type Modifiers } from "./input";
import { KeyboardRow } from "./KeyboardRow";

function Harness({ send, appCursor = false }: { send: (text: string) => void; appCursor?: boolean }) {
  const [modifiers, setModifiers] = useState<Modifiers>({ ctrl: false, alt: false });
  return (
    <>
      <KeyboardRow
        modifiers={modifiers}
        onModifiers={setModifiers}
        onSend={(text) => {
          send(applyModifiers(text, modifiers));
          if (modifiers.ctrl || modifiers.alt) setModifiers({ ctrl: false, alt: false });
        }}
        applicationCursor={() => appCursor}
      />
      {/* Stands for a letter typed on the soft keyboard: it takes the same path as the row's keys. */}
      <button type="button" onClick={() => { send(applyModifiers("c", modifiers)); if (modifiers.ctrl || modifiers.alt) setModifiers({ ctrl: false, alt: false }); }}>
        type c
      </button>
    </>
  );
}

describe("KeyboardRow", () => {
  it("has Esc, Tab, Ctrl, Alt, four arrows and the symbols | / ~ -", () => {
    render(<Harness send={() => undefined} />);
    const names = screen.getAllByRole("button").map((button) => button.getAttribute("aria-label") ?? button.textContent);
    expect(names).toEqual(["Esc", "Tab", "Ctrl", "Alt", "Left arrow", "Up arrow", "Down arrow", "Right arrow", "Symbol |", "Symbol /", "Symbol ~", "Symbol -", "type c"]);
  });

  it("sends Esc and Tab as their bytes", async () => {
    const send = vi.fn();
    render(<Harness send={send} />);
    await userEvent.click(screen.getByRole("button", { name: "Esc" }));
    await userEvent.click(screen.getByRole("button", { name: "Tab" }));
    expect(send.mock.calls).toEqual([["\x1b"], ["\t"]]);
  });

  it("arrows send ESC sequences, and the application form when the program asked for it", async () => {
    const normal = vi.fn();
    const { unmount } = render(<Harness send={normal} />);
    for (const name of ["Up arrow", "Down arrow", "Right arrow", "Left arrow"]) await userEvent.click(screen.getByRole("button", { name }));
    expect(normal.mock.calls.map(([text]) => text)).toEqual(["\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D"]);
    unmount();
    const application = vi.fn();
    render(<Harness send={application} appCursor />);
    await userEvent.click(screen.getByRole("button", { name: "Up arrow" }));
    expect(application).toHaveBeenCalledWith("\x1bOA");
  });

  it("sends the symbols as they are", async () => {
    const send = vi.fn();
    render(<Harness send={send} />);
    for (const name of ["Symbol |", "Symbol /", "Symbol ~", "Symbol -"]) await userEvent.click(screen.getByRole("button", { name }));
    expect(send.mock.calls.map(([text]) => text)).toEqual(["|", "/", "~", "-"]);
  });

  it("Ctrl is sticky: the next typed letter becomes a control character, then it turns itself off", async () => {
    const send = vi.fn();
    render(<Harness send={send} />);
    const ctrl = screen.getByRole("button", { name: "Ctrl" });
    expect(ctrl).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(ctrl);
    expect(ctrl).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "type c" }));
    expect(send).toHaveBeenLastCalledWith("\x03");
    expect(ctrl).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(screen.getByRole("button", { name: "type c" }));
    expect(send).toHaveBeenLastCalledWith("c");
  });

  it("Ctrl then Esc sends Esc once and clears the modifier; Alt prefixes the next key with ESC; Ctrl toggles off again", async () => {
    const send = vi.fn();
    render(<Harness send={send} />);
    const ctrl = screen.getByRole("button", { name: "Ctrl" });
    await userEvent.click(ctrl);
    await userEvent.click(ctrl);
    expect(ctrl).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(screen.getByRole("button", { name: "Alt" }));
    await userEvent.click(screen.getByRole("button", { name: "type c" }));
    expect(send).toHaveBeenLastCalledWith("\x1bc");
  });
});
