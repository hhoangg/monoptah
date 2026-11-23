import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SETTINGS_ANCHOR, SETTINGS_HIDE_DIVIDER_SELF_WRAPPED, SETTINGS_DIVIDER_ICON, SETTINGS_DIVIDER_PLAIN, SettingsCard, SettingsRow } from "./SettingsCard";

describe("SETTINGS_ANCHOR", () => {
  it("is the scroll margin of the settings anchor offset variable, carried by an anchored card", () => {
    expect(SETTINGS_ANCHOR).toBe("scroll-mt-[var(--settings-anchor-offset)]");
    render(<SettingsCard id="passkeys" className={SETTINGS_ANCHOR} title="Passkeys" />);
    expect(document.getElementById("passkeys")).toHaveClass(SETTINGS_ANCHOR);
  });
});

describe("SettingsCard geometry", () => {
  it("has the same inset on every side, from one variable", () => {
    render(
      <SettingsCard title="Account">
        <SettingsRow title="Profile" />
      </SettingsCard>,
    );
    const body = document.querySelector('[data-slot="settings-card"]') as HTMLElement;
    expect(body).toHaveClass("rounded-card", "bg-surface", "p-[var(--settings-card-inset)]");
    expect(body.className).not.toMatch(/\bp[xy]-/);
  });

  it("starts the divider at the text: 54px after an icon slot, 6px without, ending 6px before the right edge", () => {
    render(
      <SettingsCard>
        <SettingsRow icon="account" title="With icon" />
        <SettingsRow title="Plain" />
      </SettingsCard>,
    );
    const withIcon = screen.getByText("With icon").closest("div[class*='before:']") as HTMLElement;
    const plain = screen.getByText("Plain").closest("div[class*='before:']") as HTMLElement;
    expect(withIcon).toHaveClass("before:left-[54px]", "before:right-1.5", "first:before:hidden");
    expect(withIcon).not.toHaveClass("before:left-1.5");
    expect(plain).toHaveClass("before:left-1.5", "before:right-1.5");
    expect(plain).not.toHaveClass("before:left-[54px]");
    expect(SETTINGS_DIVIDER_ICON).toContain("before:left-[54px]");
    expect(SETTINGS_DIVIDER_PLAIN).toContain("before:left-1.5");
  });

  it("rounds the row highlight with the row radius (card radius minus the inset)", () => {
    render(
      <SettingsCard>
        <SettingsRow title="Press me" onClick={() => undefined} />
      </SettingsCard>,
    );
    expect(screen.getByRole("button", { name: "Press me" })).toHaveClass("rounded-row", "px-1.5", "py-2");
  });

  it("hides the divider of an interactive row while it is highlighted, and every row hides it after a highlighted one", () => {
    render(
      <SettingsCard>
        <SettingsRow title="First" onClick={() => undefined} />
        <SettingsRow title="Second" onClick={() => undefined} />
        <SettingsRow title="Static" />
      </SettingsCard>,
    );
    const second = screen.getByRole("button", { name: "Second" });
    expect(second).toHaveClass("hover:before:opacity-0", "focus-visible:before:opacity-0", "aria-[current=page]:before:opacity-0");
    const afterHover = "[@media(hover:hover)]:[[data-row-highlight]:hover+&]:before:opacity-0";
    const after = "[[data-row-highlight]:is(:focus-visible,[aria-current=page])+&]:before:opacity-0";
    // The hover rule carries the media condition (touch screens have no hover); focus and current page do not.
    expect(second).toHaveClass(afterHover, after);
    for (const token of second.className.split(/\s+/)) {
      if (token.includes(":hover") && token.includes("[data-row-highlight]")) expect(token.startsWith("[@media(hover:hover)]:")).toBe(true);
    }
    // Only the element that draws the highlight is marked, not a plain `a` or `button` next to it.
    expect(second).toHaveAttribute("data-row-highlight");
    // A static row has no highlight of its own, but still hides its divider under a highlighted row above it.
    const staticRow = screen.getByText("Static").closest("div[class*='before:']") as HTMLElement;
    expect(staticRow).toHaveClass(afterHover, after);
    expect(staticRow).not.toHaveClass("hover:before:opacity-0");
    expect(staticRow).not.toHaveAttribute("data-row-highlight");
  });

  it("marks the highlight element of a row, not a plain button or link next to it", () => {
    render(
      <SettingsCard>
        <li>
          <SettingsRow title="Main" onClick={() => undefined} />
          <button type="button">Trailing</button>
          <a href="/elsewhere">Elsewhere</a>
        </li>
      </SettingsCard>,
    );
    expect(screen.getByRole("button", { name: "Main" })).toHaveAttribute("data-row-highlight");
    expect(screen.getByRole("button", { name: "Trailing" })).not.toHaveAttribute("data-row-highlight");
    expect(screen.getByRole("link", { name: "Elsewhere" })).not.toHaveAttribute("data-row-highlight");
  });

  it("wraps the hover part of the wrapped-row self hide in the hover media condition", () => {
    expect(SETTINGS_HIDE_DIVIDER_SELF_WRAPPED).toContain("[@media(hover:hover)]:has-[>[data-row-highlight]:hover]:before:opacity-0");
    expect(SETTINGS_HIDE_DIVIDER_SELF_WRAPPED).toContain("has-[>[data-row-highlight]:is(:focus-visible,[aria-current=page])]:before:opacity-0");
    expect(SETTINGS_HIDE_DIVIDER_SELF_WRAPPED).not.toContain(":is(a,button)");
  });
});
