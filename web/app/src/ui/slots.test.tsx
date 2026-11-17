import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { clearSlots, getSlot, registerSlot, Slot, SLOT_NAMES } from "./slots";

afterEach(clearSlots);

describe("slots registry", () => {
  it("declares every slot name of workstreams.md 3.2", () => {
    expect([...SLOT_NAMES].sort()).toEqual(
      [
        "composer-top",
        "session-card-badge",
        "session-header-chip",
        "session-menu-items",
        "archive-dialog",
        "send-guard",
        "new-session-target",
        "desktop-nav-items",
        "session-capabilities",
        "desktop-settings-sections",
        "desktop-switcher",
        "link-state-screens",
        "first-run-picker",
        "app-shell",
        "settings-sections",
      ].sort(),
    );
  });

  it("renders nothing for an unregistered slot", () => {
    const { container } = render(<Slot name="composer-top">fallback</Slot>);
    expect(container).toBeEmptyDOMElement();
    expect(getSlot("composer-top")).toBeUndefined();
  });

  it("renders the registered component with the slot props", () => {
    registerSlot("session-header-chip", ({ label }) => <span>chip:{String(label)}</span>);
    render(<Slot name="session-header-chip" label="cloud" />);
    expect(screen.getByText("chip:cloud")).toBeInTheDocument();
  });

  it("app-shell passes its children through by default", () => {
    render(
      <Slot name="app-shell">
        <p>content</p>
      </Slot>,
    );
    expect(screen.getByText("content")).toBeInTheDocument();
  });

  it("app-shell can be replaced and receives the children", () => {
    registerSlot("app-shell", ({ children }) => <div data-testid="custom-shell">{children}</div>);
    render(
      <Slot name="app-shell">
        <p>content</p>
      </Slot>,
    );
    expect(screen.getByTestId("custom-shell")).toContainElement(screen.getByText("content"));
  });

  it("a second registration replaces the first", () => {
    registerSlot("link-state-screens", () => <span>first</span>);
    registerSlot("link-state-screens", () => <span>second</span>);
    render(<Slot name="link-state-screens" />);
    expect(screen.queryByText("first")).toBeNull();
    expect(screen.getByText("second")).toBeInTheDocument();
  });

  it("new-session-target is a controlled slot with value and onChange", () => {
    let seen: unknown = "unset";
    registerSlot("new-session-target", ({ value, onChange }) => (
      <button onClick={() => onChange?.({ kind: "cloud" })}>{String(value)}</button>
    ));
    render(<Slot name="new-session-target" value={null} onChange={(t) => (seen = t)} />);
    screen.getByRole("button", { name: "null" }).click();
    expect(seen).toEqual({ kind: "cloud" });
  });
});
