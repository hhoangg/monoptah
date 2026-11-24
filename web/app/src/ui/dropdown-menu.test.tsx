import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu";

describe("DropdownMenuContent", () => {
  it("uses the shared menu surface, scrolls inside the room Radix measures and marks its items", () => {
    render(
      <DropdownMenu open>
        <DropdownMenuTrigger>Open</DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem>First</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    const content = screen.getByRole("menu");
    expect(content).toHaveAttribute("data-menu-surface");
    expect(content).toHaveClass("rounded-card", "border", "overflow-y-auto", "max-h-[var(--radix-dropdown-menu-content-available-height)]", "p-[calc(var(--settings-card-inset)-1px)]");
    expect(content).not.toHaveClass("overflow-hidden", "p-1.5");
    const item = screen.getByRole("menuitem", { name: "First" });
    expect(item).toHaveAttribute("data-menu-item");
    expect(item).toHaveClass("rounded-row", "data-[highlighted]:bg-selection");
  });
});
