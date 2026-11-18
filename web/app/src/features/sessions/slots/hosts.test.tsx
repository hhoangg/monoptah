import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { SessionCard } from "@monoptah/protocol";
import { clearSlots, registerSlot } from "@/ui/slots";
import { ArchiveDialogSlot, ComposerTopSlot, SessionCardBadgeSlot, SessionHeaderChipSlot, SessionMenuItemsSlot } from "./index";

afterEach(clearSlots);

const session: SessionCard = { id: "s1", projectId: "p1", title: "T", harness: "claude", model: "m", branch: "main", updatedAt: 1 };

describe("WS-T slot hosts", () => {
  it("render nothing by default, and the fallback where a host has one", () => {
    const view = render(
      <div>
        <ComposerTopSlot desktopId="d" sessionId="s1" />
        <SessionCardBadgeSlot desktopId="d" session={session} />
        <SessionHeaderChipSlot desktopId="d" session={session} />
        <SessionMenuItemsSlot desktopId="d" session={session} />
        <ArchiveDialogSlot desktopId="d" session={session} open onOpenChange={() => {}} fallback={<p>plain archive</p>} />
      </div>,
    );
    expect(view.container.textContent).toBe("plain archive");
  });

  it("render the registered component with the host's props", () => {
    registerSlot("session-card-badge", (props) => <span>badge {String((props.session as SessionCard).id)}</span>);
    registerSlot("composer-top", (props) => <span>top {String(props.sessionId)}</span>);
    registerSlot("session-header-chip", () => <span>chip</span>);
    registerSlot("session-menu-items", () => <span>menu</span>);
    registerSlot("archive-dialog", (props) => <span>archive {String(props.open)}</span>);
    render(
      <div>
        <ComposerTopSlot desktopId="d" sessionId="s1" />
        <SessionCardBadgeSlot desktopId="d" session={session} />
        <SessionHeaderChipSlot desktopId="d" session={session} />
        <SessionMenuItemsSlot desktopId="d" session={session} />
        <ArchiveDialogSlot desktopId="d" session={session} open onOpenChange={() => {}} fallback={<p>plain archive</p>} />
      </div>,
    );
    for (const text of ["badge s1", "top s1", "chip", "menu", "archive true"]) expect(screen.getByText(text)).toBeInTheDocument();
    expect(screen.queryByText("plain archive")).toBeNull();
  });
});
