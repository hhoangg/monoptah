import { SendableRuntimeModeSchema } from "@monoptah/protocol";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { AccessPicker } from "./AccessPicker";
import { ACCESS_MODES, DEFAULT_ACCESS, FULL_ACCESS_DISPLAY, accessMode, sendableAccess } from "./access";

beforeAll(() => {
  // Radix menus use pointer capture and scrollIntoView, which jsdom lacks.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

describe("access choices", () => {
  it("match the protocol's sendable modes exactly and never include full-access", () => {
    expect(ACCESS_MODES.map((mode) => mode.id)).toEqual([...SendableRuntimeModeSchema.options]);
    expect(ACCESS_MODES.map((mode) => mode.id)).not.toContain("full-access");
    expect(JSON.stringify(ACCESS_MODES)).not.toMatch(/full.access/i);
  });

  it("cannot turn full-access into something sendable", () => {
    expect(sendableAccess("full-access")).toBeNull();
    expect(sendableAccess("whatever")).toBeNull();
    expect(accessMode("full-access").id).not.toBe("full-access");
    expect(DEFAULT_ACCESS).toBe("supervised");
    for (const mode of ACCESS_MODES) expect(SendableRuntimeModeSchema.safeParse(sendableAccess(mode.id)).success).toBe(true);
  });
});

describe("AccessPicker", () => {
  it("lists the three sendable modes and reports a choice as a sendable mode", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<AccessPicker value="supervised" onChange={onChange} />);
    await user.click(screen.getByRole("button", { name: "Access: Supervised" }));
    const menu = await screen.findByRole("menu");
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((item) => item.textContent)).toEqual([
      expect.stringContaining("Supervised"),
      expect.stringContaining("Auto-accept edits"),
      expect.stringContaining("Auto"),
    ]);
    expect(items[0]).toHaveAttribute("aria-checked", "true");
    expect(within(menu).queryByText(/^Full access$/)).toBeNull();
    await user.click(items[1]!);
    expect(onChange).toHaveBeenCalledWith("auto-accept-edits");
  });

  it("shows full access read only, with nothing selected and no way to send it", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<AccessPicker value="full-access" onChange={onChange} />);
    const trigger = screen.getByRole("button", { name: "Access: Full access" });
    await user.click(trigger);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByText(FULL_ACCESS_DISPLAY.note)).toBeInTheDocument();
    for (const item of within(menu).getAllByRole("menuitemradio")) {
      expect(item).toHaveAttribute("aria-checked", "false");
    }
    await user.click(within(menu).getAllByRole("menuitemradio")[2]!);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith("auto");
  });

  it("is disabled for a read-only device", () => {
    render(<AccessPicker value="supervised" onChange={vi.fn()} disabled />);
    expect(screen.getByRole("button", { name: "Access: Supervised" })).toBeDisabled();
  });

  it("opens from the keyboard and picks with Enter", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<AccessPicker value="supervised" onChange={onChange} />);
    screen.getByRole("button", { name: "Access: Supervised" }).focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalled();
    expect(["supervised", "auto-accept-edits", "auto"]).toContain(onChange.mock.calls[0]![0]);
  });
});
