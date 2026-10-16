// @vitest-environment happy-dom
// Keep this as .ts because the project test glob intentionally excludes .test.tsx.
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../features/contributors/model/api", () => ({
  startSync: vi.fn(),
  cancelSync: vi.fn(),
  syncStatus: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { listen } from "@tauri-apps/api/event";
import { syncStatus } from "../../features/contributors/model/api";
import type { SyncEvent } from "../../features/contributors/model/syncEvents";
import {
  getSyncJobs,
  resetSyncJobs,
} from "../../features/contributors/model/syncJobs";
import { UsageFooter } from "./UsageFooter";

const event = (over: Partial<SyncEvent> = {}): SyncEvent => ({
  repoKey: "/r/.git",
  project: "alpha",
  root: "/work/alpha",
  branch: "main",
  from: 1,
  to: 2,
  step: "commits",
  done: 3200,
  total: 12000,
  percent: 42,
  state: "running",
  error: null,
  ...over,
});

let handlers: ((e: { payload: SyncEvent }) => void)[] = [];
let container: HTMLDivElement;
let root: Root;
const onOpenContributors = vi.fn();

const emit = (e: SyncEvent) =>
  act(async () => {
    handlers.forEach((h) => h({ payload: e }));
  });
const mount = () =>
  act(async () => {
    root.render(
      createElement(UsageFooter, {
        providers: [],
        onNewTerminal: vi.fn(),
        onOpenContributors,
      }),
    );
  });
const indicator = () =>
  container.querySelector<HTMLButtonElement>(
    'button[data-testid="contributors-sync"]',
  );

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  resetSyncJobs();
  handlers = [];
  onOpenContributors.mockReset();
  vi.mocked(listen).mockImplementation((async (
    _name: string,
    handler: (e: { payload: SyncEvent }) => void,
  ) => {
    handlers.push(handler);
    return () => {
      handlers = handlers.filter((h) => h !== handler);
    };
  }) as unknown as typeof listen);
  vi.mocked(syncStatus).mockResolvedValue([]);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  resetSyncJobs();
  vi.unstubAllGlobals();
});

describe("footer contributors sync indicator", () => {
  it("renders nothing while no sync runs", async () => {
    await mount();
    expect(indicator()).toBeNull();
    expect(container.textContent).toContain("Terminal");
  });

  it("shows the analytics icon and the percent of a running job", async () => {
    await mount();
    await emit(event());
    const button = indicator();
    expect(button?.textContent).toBe("42%");
    expect(button?.querySelector("svg")).not.toBeNull();
    expect(button?.title).toBe(
      "Syncing contributors · alpha · Commits 3,200/12,000",
    );
    expect(button?.className).not.toContain("red");
  });

  it("sits left of the Terminal button", async () => {
    await mount();
    await emit(event());
    const buttons = [...container.querySelectorAll("button")];
    const sync = buttons.indexOf(indicator()!);
    const terminal = buttons.findIndex((b) => b.textContent === "Terminal");
    expect(sync).toBeGreaterThan(-1);
    expect(terminal).toBeGreaterThan(sync);
  });

  it("combines two jobs and lists each in the tooltip", async () => {
    await mount();
    await emit(event({ percent: 20 }));
    await emit(
      event({
        root: "/work/beta",
        project: "beta",
        percent: 60,
        done: 1,
        total: 4,
        step: "github",
      }),
    );
    expect(indicator()?.textContent).toBe("40%");
    expect(indicator()?.title.split("\n")).toEqual([
      "Syncing contributors · alpha · Commits 3,200/12,000",
      "Syncing contributors · beta · GitHub 1/4",
    ]);
  });

  it("goes away when the job is done", async () => {
    await mount();
    await emit(event());
    await emit(event({ state: "done", percent: 100 }));
    expect(indicator()).toBeNull();
  });

  it("opens the Contributors page of the first job when clicked", async () => {
    await mount();
    await emit(event());
    await emit(event({ root: "/work/beta", from: 5 }));
    await act(async () => indicator()!.click());
    expect(onOpenContributors).toHaveBeenCalledWith("/work/alpha");
    // A running job stays on screen.
    expect(indicator()).not.toBeNull();
  });

  it("turns red on failure and stays until clicked, which opens the page and dismisses", async () => {
    await mount();
    await emit(event());
    await emit(event({ state: "error", error: "git exploded", percent: 42 }));
    expect(indicator()?.className).toContain("red");
    expect(indicator()?.title).toBe(
      "Contributors sync stopped · alpha · git exploded",
    );
    await act(async () => indicator()!.click());
    expect(onOpenContributors).toHaveBeenCalledWith("/work/alpha");
    expect(indicator()).toBeNull();
    expect(Object.keys(getSyncJobs())).toHaveLength(0);
  });

  it("shows a job that was already running when the app reloaded", async () => {
    vi.mocked(syncStatus).mockResolvedValue([event({ percent: 77 })]);
    await mount();
    expect(indicator()?.textContent).toBe("77%");
  });

  it("does not show a job Rust no longer lists after a reload", async () => {
    vi.mocked(syncStatus).mockResolvedValue([]);
    await mount();
    expect(indicator()).toBeNull();
  });
});
