import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MatchText } from "./MatchText";

describe("MatchText", () => {
  it("accents matched runs as spans and keeps the full text", () => {
    const { container } = render(<span><MatchText text="readme" positions={[0, 1, 4]} active /></span>);
    expect(container.textContent).toBe("readme");
    const marked = [...container.querySelectorAll("[data-match]")].map((el) => el.textContent);
    expect(marked).toEqual(["re", "m"]);
    expect(container.querySelector("[data-match]")).toHaveClass("text-accent");
  });

  it("renders plain text when inactive or without positions", () => {
    const a = render(<span><MatchText text="abc" positions={[0]} active={false} /></span>);
    expect(a.container.querySelector("[data-match]")).toBeNull();
    const b = render(<span><MatchText text="abc" positions={[]} active /></span>);
    expect(b.container.querySelector("[data-match]")).toBeNull();
    expect(b.container.textContent).toBe("abc");
  });

  it("renders markup in the text as literal text", () => {
    const { container } = render(<span><MatchText text="<b>x</b>" positions={[1]} active /></span>);
    expect(container.querySelector("b")).toBeNull();
    expect(container.textContent).toBe("<b>x</b>");
  });
});
