import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PageHeader } from "./PageHeader";

describe("PageHeader alignment", () => {
  it("centres a leading control and a single-line title when there is no description", () => {
    render(<PageHeader title="Settings" leading={<button type="button">Leave</button>} />);
    const header = screen.getByRole("banner");
    expect(header).toHaveClass("items-center");
    expect(header).not.toHaveClass("items-start");
  });

  it("keeps the top alignment when there is a description", () => {
    render(<PageHeader title="Settings" description="Manage things" leading={<button type="button">Leave</button>} />);
    const header = screen.getByRole("banner");
    expect(header).toHaveClass("items-start");
    expect(header).not.toHaveClass("items-center");
  });
});
