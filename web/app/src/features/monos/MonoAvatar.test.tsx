import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MonoAvatar, isKnownMascot, monoColor } from "./MonoAvatar";

describe("MonoAvatar", () => {
  it("draws the pixel mascot the desktop picked, in its color", () => {
    const { container } = render(<MonoAvatar monoId="m1" name="Bob" mascot="cat" color="#39c363" size={40} />);
    expect(screen.getByTestId("mono-avatar")).toHaveAttribute("data-kind", "mascot");
    const svg = container.querySelector("svg");
    expect(svg).toHaveAttribute("data-mascot", "cat");
    expect(svg?.querySelector("path")).toHaveAttribute("fill", "#39c363");
    expect(container.textContent).toBe("");
  });

  it("falls back to the initial in a colored tile when the desktop sends no mascot (an older desktop)", () => {
    render(<MonoAvatar monoId="m1" name="Sam" color="#f4bf25" />);
    const tile = screen.getByTestId("mono-avatar");
    expect(tile).toHaveAttribute("data-kind", "initial");
    expect(tile).toHaveTextContent("S");
    expect(tile).toHaveStyle({ backgroundColor: "#f4bf25" });
  });

  it("treats a mascot name this web does not have as missing", () => {
    render(<MonoAvatar monoId="m1" name="Zed" mascot="unicorn" color="#ea603e" />);
    expect(screen.getByTestId("mono-avatar")).toHaveAttribute("data-kind", "initial");
    expect(isKnownMascot("unicorn")).toBe(false);
    expect(isKnownMascot("frog")).toBe(true);
    expect(isKnownMascot(undefined)).toBe(false);
  });

  it("never puts a color that is not #rrggbb into a style", () => {
    expect(monoColor("Bob", "red; background:url(x)")).not.toContain("url");
    expect(monoColor("Bob", "#112233")).toBe("#112233");
  });
});
