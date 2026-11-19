import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PathLabel } from "./PathLabel";

describe("PathLabel", () => {
  it("puts the file name first and the folder after it, with the full path as the title", () => {
    render(<PathLabel path="src/features/files/FileViewer.tsx" />);
    const label = screen.getByTitle("src/features/files/FileViewer.tsx");
    expect(label.children[0]).toHaveTextContent("FileViewer.tsx");
    expect(label.children[0]).toHaveClass("shrink-0");
    expect(label.children[1]).toHaveTextContent("src/features/files");
    expect(label.children[1]).toHaveClass("truncate");
  });

  it("shows only the name for a file in the root", () => {
    render(<PathLabel path="README.md" />);
    expect(screen.getByTitle("README.md").children).toHaveLength(1);
  });

  it("accents matched characters in the name and the folder", () => {
    const { container } = render(<PathLabel path="src/a.ts" positions={[0, 4]} />);
    const matches = Array.from(container.querySelectorAll("[data-match]")).map((node) => node.textContent);
    expect(matches).toEqual(["a", "s"]);
  });
});
