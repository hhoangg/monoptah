import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "./contextMenu";

describe("ContextMenuContent", () => {
  it("uses the shared menu surface, scrolls inside the measured room and marks its items", async () => {
    render(
      <ContextMenu>
        <ContextMenuTrigger>Target</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem>First</ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem destructive>Delete</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>,
    );
    fireEvent.contextMenu(screen.getByText("Target"));
    const content = await screen.findByRole("menu");
    expect(content).toHaveAttribute("data-menu-surface");
    expect(content).toHaveClass("rounded-card", "border", "overflow-y-auto", "max-h-[var(--radix-context-menu-content-available-height)]", "p-[calc(var(--settings-card-inset)-1px)]");
    expect(content).not.toHaveClass("overflow-hidden", "p-1.5");
    const item = screen.getByRole("menuitem", { name: "First" });
    expect(item).toHaveAttribute("data-menu-item");
    expect(item).toHaveClass("rounded-row", "data-[highlighted]:bg-selection");
    expect(screen.getByRole("menuitem", { name: "Delete" })).toHaveClass("text-deletions");
  });
});
