import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Icon, ICONS, type IconName } from "./Icon";

describe("Icon", () => {
  const names = Object.keys(ICONS) as IconName[];

  it("maps the 50 mobile names and the 9 web-only names of design-system.md section 7", () => {
    expect(names).toHaveLength(59);
  });

  it.each(names)("renders %s as an svg with the default size and stroke", (name) => {
    // A missing Hugeicons export makes ICONS[name] undefined and HugeiconsIcon renders nothing useful.
    expect(ICONS[name]).toBeDefined();
    expect(Array.isArray(ICONS[name])).toBe(true);
    const { container, unmount } = render(<Icon name={name} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg).toHaveAttribute("width", "20");
    expect(svg).toHaveAttribute("height", "20");
    expect(svg?.querySelectorAll("path, circle, rect, line, polyline, ellipse").length).toBeGreaterThan(0);
    expect(svg?.querySelector("[stroke-width]")).toHaveAttribute("stroke-width", "1.75");
    unmount();
  });

  it("is decorative unless labelled", () => {
    const { container, rerender } = render(<Icon name="search" />);
    expect(container.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    rerender(<Icon name="search" aria-label="Search" size={22} />);
    const svg = container.querySelector("svg");
    expect(svg).toHaveAttribute("role", "img");
    expect(svg).toHaveAttribute("width", "22");
  });
});
