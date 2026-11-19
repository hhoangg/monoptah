import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Shimmer } from "./Shimmer";

describe("Shimmer", () => {
  it("exposes its text and sizes the sweep from the text length", () => {
    render(<Shimmer>Thinking</Shimmer>);
    const el = screen.getByText("Thinking");
    expect(el.className).toContain("shimmer-text");
    expect(el.style.getPropertyValue("--spread")).toBe("16px");
    expect(el.style.getPropertyValue("--shimmer-duration")).toBe("2s");
  });
});
