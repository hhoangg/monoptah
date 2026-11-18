import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FileTypeIcon, iconForKind } from "./FileTypeIcon";

describe("FileTypeIcon", () => {
  it("renders an svg icon chosen by kind, with no raw HTML injection", () => {
    const { container } = render(<FileTypeIcon name="src/main.ts" isDir={false} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg).toHaveAttribute("data-icon", "source");
    expect(container.firstElementChild).toHaveAttribute("aria-hidden", "true");
    expect(container.firstElementChild).toHaveAttribute("data-kind", "code");
  });

  it("uses the folder icon for directories, open or closed", () => {
    const closed = render(<FileTypeIcon name="src" isDir />);
    expect(closed.container.querySelector("svg")).toHaveAttribute("data-icon", "folder");
    const open = render(<FileTypeIcon name="src" isDir isOpen />);
    expect(open.container.firstElementChild).toHaveAttribute("data-kind", "folder-open");
  });

  it("falls back to the generic file icon and honours size", () => {
    const { container } = render(<FileTypeIcon name="noext" isDir={false} size={22} />);
    expect(container.querySelector("svg")).toHaveAttribute("data-icon", "file");
    expect(container.querySelector("svg")).toHaveAttribute("width", "22");
    expect(iconForKind("secret")).toBe("key");
  });
});
