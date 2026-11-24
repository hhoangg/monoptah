import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type * as RadixUi from "radix-ui";
import { forwardRef, type ComponentProps, type ElementType } from "react";
import { describe, expect, it, vi } from "vitest";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/features/monos/contextMenu";
import { VISIBLE_MARGIN_PX } from "./anchoredPlacement";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu";

// The menus pass collisionPadding to Radix, which keeps it out of the DOM: wrap the real Content to record what it receives.
const captured = vi.hoisted(() => [] as Array<{ kind: string; collisionPadding: unknown }>);
vi.mock("radix-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof RadixUi>();
  const wrap = (kind: string, Real: ElementType) =>
    forwardRef<HTMLDivElement, ComponentProps<"div"> & { collisionPadding?: unknown }>(function Wrapped(props, ref) {
      captured.push({ kind, collisionPadding: props.collisionPadding });
      return <Real ref={ref} {...props} />;
    });
  return {
    ...actual,
    DropdownMenu: { ...actual.DropdownMenu, Content: wrap("dropdown", actual.DropdownMenu.Content) },
    ContextMenu: { ...actual.ContextMenu, Content: wrap("context", actual.ContextMenu.Content) },
  };
});

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

describe("menu collision padding", () => {
  it("dropdown and context menu default to the shared visible margin, and a caller can override it", async () => {
    captured.length = 0;
    render(
      <DropdownMenu open>
        <DropdownMenuTrigger>Open</DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem>One</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    expect(captured.at(-1)).toEqual({ kind: "dropdown", collisionPadding: VISIBLE_MARGIN_PX });
    cleanup();
    render(
      <ContextMenu>
        <ContextMenuTrigger>Target</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem>One</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>,
    );
    fireEvent.contextMenu(screen.getByText("Target"));
    await screen.findByRole("menu");
    expect(captured.at(-1)).toEqual({ kind: "context", collisionPadding: VISIBLE_MARGIN_PX });
    cleanup();
    render(
      <DropdownMenu open>
        <DropdownMenuTrigger>Open</DropdownMenuTrigger>
        <DropdownMenuContent collisionPadding={3}>
          <DropdownMenuItem>One</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>,
    );
    expect(captured.at(-1)).toEqual({ kind: "dropdown", collisionPadding: 3 });
  });
});
