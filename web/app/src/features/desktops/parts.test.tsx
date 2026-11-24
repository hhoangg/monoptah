import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ColorDot, PresenceBadge, PresenceDot } from "./parts";

describe("ColorDot", () => {
  it("draws a readable ring (muted-text, never faint) when the desktop has no color", () => {
    const { container } = render(<ColorDot color={null} />);
    const dot = container.firstElementChild!;
    expect(dot).toHaveClass("border", "border-muted-text");
    expect(dot).not.toHaveClass("border-faint");
  });
});

describe("PresenceDot", () => {
  it("is green online and muted-text offline, never faint", () => {
    const online = render(<PresenceDot online />).container.firstElementChild!;
    expect(online).toHaveClass("bg-additions");
    const offline = render(<PresenceDot online={false} />).container.firstElementChild!;
    expect(offline).toHaveClass("bg-muted-text");
    expect(offline).not.toHaveClass("bg-faint");
  });

  it("is the dot inside PresenceBadge", () => {
    const { container } = render(<PresenceBadge online={false} />);
    expect(container.querySelector('span[aria-hidden="true"]')).toHaveClass("bg-muted-text");
    expect(container).toHaveTextContent("Offline");
  });
});
