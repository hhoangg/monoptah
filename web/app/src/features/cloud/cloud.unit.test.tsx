// U-16 pure parts: the step list per status, the chip per kind and state, the send guard per state.
import { render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MOVE_STEP_IDS, STARTUP_STEP_IDS, STEP_STATUSES, type CloudState, type StepStatus } from "@monoptah/protocol";
import { COST_NOTICES, STARTING_REFUSAL, UNAVAILABLE_REFUSAL } from "./copy";
import { KIND_LABELS, STATE_LABELS, placementOf, placementSources } from "./model";
import { CloudChip } from "./session/CloudChip";
import { cloudSendGuard } from "./session/guard";
import { clearLivePlacements, publishPlacement } from "./session/liveState";
import { StepList, currentStepText, moveItems, startupItems, startupItemsFromCard } from "./StepList";

afterEach(clearLivePlacements);

describe("StepList", () => {
  it.each(STEP_STATUSES)("says the %s status in words", (status: StepStatus) => {
    render(<StepList title="Steps" steps={[{ id: "a", label: "First", status }]} />);
    const row = within(screen.getByRole("list", { name: "Steps" })).getByRole("listitem");
    expect(row).toHaveTextContent(status[0]!.toUpperCase() + status.slice(1));
    expect(row).toHaveAttribute("data-status", status);
  });

  it("announces the running step politely, and nothing when none runs", () => {
    const { rerender } = render(<StepList title="Steps" steps={[{ id: "a", label: "Build the container", status: "running" }]} />);
    expect(screen.getByRole("status")).toHaveAttribute("aria-live", "polite");
    expect(screen.getByRole("status")).toHaveTextContent("Step 1 of 1: Build the container, running");
    rerender(<StepList title="Steps" steps={[{ id: "a", label: "Build", status: "done" }]} />);
    expect(currentStepText([{ id: "a", label: "Build", status: "done" }])).toBe("");
  });

  it("lists the 9 startup steps in protocol order, filling in missing ones as pending", () => {
    const items = startupItems({ steps: [{ id: "provision", status: "running" }] });
    expect(items.map((item) => item.id)).toEqual([...STARTUP_STEP_IDS]);
    expect(items.map((item) => item.status)).toEqual(["pending", "running", ...Array(7).fill("pending")]);
  });

  it("rebuilds the startup steps from the catalog card", () => {
    const items = startupItemsFromCard("buildContainer", true);
    expect(items.map((item) => item.status)).toEqual(["done", "done", "failed", ...Array(6).fill("pending")]);
  });

  it("lists the 7 move steps with the earlier ones done", () => {
    const items = moveItems({ step: "restoreCode", status: "failed" });
    expect(items.map((item) => item.id)).toEqual([...MOVE_STEP_IDS]);
    expect(items.map((item) => item.status)).toEqual(["done", "done", "done", "done", "failed", "pending", "pending"]);
  });
});

describe("CloudChip", () => {
  const states = Object.keys(STATE_LABELS) as CloudState[];
  it.each(["sandbox", "codespace", "remote"] as const)("shows the %s kind label with every state as text", (kind) => {
    for (const state of states) {
      const { unmount } = render(<CloudChip placement={{ kind, state }} />);
      const chip = screen.getByTestId("cloud-chip");
      expect(chip).toHaveTextContent(KIND_LABELS[kind]);
      expect(chip).toHaveTextContent(STATE_LABELS[state]);
      unmount();
    }
  });

  it("reads Not connected for unknown and shows nothing for a local or absent placement", () => {
    expect(STATE_LABELS.unknown).toBe("Not connected");
    const { container } = render(
      <div>
        <CloudChip placement={{ kind: "local", state: undefined }} />
        <CloudChip placement={null} />
      </div>,
    );
    expect(container.querySelector("[data-testid=cloud-chip]")).toBeNull();
  });

  it("takes the live cloud state over the catalog card", () => {
    const card = { target: { kind: "sandbox" as const, state: "stopped" as const }, cloudState: "stopped" as const };
    expect(placementOf(card)).toEqual({ kind: "sandbox", state: "stopped" });
    expect(placementOf(card, { kind: "sandbox", instance: "x", state: "starting" })).toEqual({ kind: "sandbox", state: "starting" });
    expect(placementOf({})).toBeNull();
  });
});

describe("cloudSendGuard", () => {
  const send = (kind: string | null, state: CloudState | undefined, sessionId = "s1") =>
    cloudSendGuard({ desktopId: "d", sessionId, target: kind ? ({ kind, state } as { kind: string }) : null });

  it("allows a local or absent target and an available instance", async () => {
    expect(await send(null, undefined)).toEqual({ kind: "allow" });
    expect(await send("local", undefined)).toEqual({ kind: "allow" });
    expect(await send("remote", "stopped")).toEqual({ kind: "allow" });
    expect(await send("sandbox", "available")).toEqual({ kind: "allow" });
  });

  it("refuses starting and unavailable with an explanation", async () => {
    expect(await send("sandbox", "starting")).toEqual({ kind: "refuse", reason: STARTING_REFUSAL });
    expect(await send("codespace", "unavailable")).toEqual({ kind: "refuse", reason: UNAVAILABLE_REFUSAL });
  });

  // X-11: stopped, stopping and unknown ask before the send can wake a billable instance.
  it.each([
    ["stopped", COST_NOTICES.wake],
    ["stopping", COST_NOTICES.wakeStopping],
    ["unknown", COST_NOTICES.wakeUnknown],
  ] as const)("confirms the wake cost for a %s instance", async (state, notice) => {
    expect(await send("sandbox", state)).toEqual({ kind: "confirm", notice });
  });

  it("confirms a billable session whose state is not known at all", async () => {
    expect(await send("codespace", undefined)).toEqual({ kind: "confirm", notice: COST_NOTICES.wakeUnknown });
  });

  it("confirms the start cost for a cloud start from the New session page (empty session id)", async () => {
    expect(await send("codespace", undefined, "")).toEqual({ kind: "confirm", notice: COST_NOTICES.start });
    expect(await send("local", undefined, "")).toEqual({ kind: "allow" });
  });

  describe("persisted catalog (not refreshed yet)", () => {
    const card = { target: { kind: "codespace", state: "available" }, cloudState: "available" } as never;
    it("does not trust a cloud card as available without a live field", async () => {
      const sources = placementSources(card, undefined, false);
      expect(sources.states).toContain("unknown");
      publishPlacement("d", "s1", "w1", sources);
      expect(await send("codespace", "available")).toEqual({ kind: "confirm", notice: COST_NOTICES.wakeUnknown });
    });

    it("trusts it once the catalog is fresh, or when a live field arrived", () => {
      expect(placementSources(card, undefined, true).states).toEqual(["available"]);
      expect(placementSources(card, { kind: "codespace", instance: "i", state: "available" }, false).states).toEqual(["available"]);
    });

    it("leaves a local card alone", () => {
      expect(placementSources({ target: { kind: "local" } } as never, undefined, false).states).toEqual([]);
    });
  });

  describe("live sources", () => {
    it("uses the live state when the catalog card is missing", async () => {
      publishPlacement("d", "s1", "w1", { kinds: ["sandbox"], states: ["stopped"] });
      expect(await send(null, undefined)).toEqual({ kind: "confirm", notice: COST_NOTICES.wake });
    });

    it("uses the card cloudState published next to the target (a card with only cloudState)", async () => {
      publishPlacement("d", "s1", "w1", { kinds: ["codespace"], states: ["stopped"] });
      expect(await send("codespace", undefined)).toEqual({ kind: "confirm", notice: COST_NOTICES.wake });
    });

    it("takes the most cautious answer when the sources disagree", async () => {
      publishPlacement("d", "s1", "w1", { kinds: ["sandbox"], states: ["available"] });
      expect(await send("sandbox", "stopped")).toEqual({ kind: "confirm", notice: COST_NOTICES.wake });
      publishPlacement("d", "s1", "w2", { kinds: ["sandbox"], states: ["starting"] });
      expect(await send("sandbox", "stopped")).toEqual({ kind: "refuse", reason: STARTING_REFUSAL });
    });

    it("treats a session that moved to the cloud as billable although the card still says local", async () => {
      publishPlacement("d", "s1", "w1", { kinds: ["sandbox"], states: ["stopped"] });
      expect(await send("local", undefined)).toEqual({ kind: "confirm", notice: COST_NOTICES.wake });
    });

    it("keys the live state by desktop and session", async () => {
      publishPlacement("other-desktop", "s1", "w1", { kinds: ["sandbox"], states: ["stopped"] });
      publishPlacement("d", "s2", "w1", { kinds: ["sandbox"], states: ["stopped"] });
      expect(await send(null, undefined)).toEqual({ kind: "allow" });
    });

    it("keeps another writer's entry when one writer is removed (reference counted)", async () => {
      publishPlacement("d", "s1", "chip", { kinds: ["sandbox"], states: ["stopped"] });
      publishPlacement("d", "s1", "top", { kinds: ["sandbox"], states: ["stopped"] });
      publishPlacement("d", "s1", "chip", undefined);
      expect(await send(null, undefined)).toEqual({ kind: "confirm", notice: COST_NOTICES.wake });
      publishPlacement("d", "s1", "top", undefined);
      expect(await send(null, undefined)).toEqual({ kind: "allow" });
    });
  });
});
