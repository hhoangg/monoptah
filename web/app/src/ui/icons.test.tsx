import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import * as icons from "./icons";

describe("icons", () => {
  it("renders every exported glyph as an svg", () => {
    const glyphs = Object.entries(icons).filter(([, value]) => typeof value === "object" && value !== null);
    expect(glyphs.length).toBeGreaterThanOrEqual(14);
    for (const [name, Glyph] of glyphs) {
      const Component = Glyph as unknown as (props: { className?: string }) => React.ReactElement;
      const { container, unmount } = render(<Component className="size-4" />);
      expect(container.querySelector("svg"), name).not.toBeNull();
      unmount();
    }
  });
});
