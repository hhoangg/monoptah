import { describe, expect, it } from "vitest";
import type { SyncEvent } from "./syncEvents";
import { summarizeSync } from "./syncIndicator";
import { eventKey, type SyncJobs } from "./syncJobs";

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
  percent: 40,
  state: "running",
  error: null,
  ...over,
});

const jobsOf = (...events: SyncEvent[]): SyncJobs =>
  Object.fromEntries(events.map((e) => [eventKey(e), e]));

describe("summarizeSync", () => {
  it("is null while nothing runs and nothing failed", () => {
    expect(summarizeSync({})).toBeNull();
  });

  it("shows one running job as its percent and one tooltip line", () => {
    const summary = summarizeSync(jobsOf(event()));
    expect(summary).toMatchObject({
      percent: 40,
      failed: null,
      title: "Syncing contributors · alpha · Commits 3,200/12,000",
    });
    expect(summary?.target.root).toBe("/work/alpha");
  });

  it("averages several jobs equally and lists each one", () => {
    const summary = summarizeSync(
      jobsOf(
        event({ percent: 10 }),
        event({
          root: "/work/beta",
          project: "beta",
          from: 5,
          step: "pullRequests",
          done: 1,
          total: 4,
          percent: 31,
        }),
      ),
    );
    expect(summary?.percent).toBe(21);
    expect(summary?.title.split("\n")).toEqual([
      "Syncing contributors · alpha · Commits 3,200/12,000",
      "Syncing contributors · beta · Pull requests 1/4",
    ]);
    expect(summary?.target.root).toBe("/work/alpha");
  });

  it("counts a queued job as 0 percent and says it is queued", () => {
    const summary = summarizeSync(
      jobsOf(
        event({ percent: 50 }),
        event({
          from: 9,
          state: "queued",
          step: "counting",
          done: 0,
          total: 0,
          percent: 0,
        }),
      ),
    );
    expect(summary?.percent).toBe(25);
    expect(summary?.title.split("\n")[1]).toBe(
      "Syncing contributors · alpha · Queued",
    );
  });

  it("leaves out the counts while a step has no total yet", () => {
    const summary = summarizeSync(
      jobsOf(event({ step: "counting", done: 0, total: 0, percent: 1 })),
    );
    expect(summary?.title).toBe("Syncing contributors · alpha · Counting");
  });

  it("falls back to the folder of the path when the project name is unknown", () => {
    const summary = summarizeSync(
      jobsOf(event({ project: "", root: "/work/gamma" })),
    );
    expect(summary?.title).toContain("· gamma ·");
  });

  it("keeps a failure on top, with its error, until it is dismissed", () => {
    const failed = event({
      from: 7,
      state: "error",
      error: "git exploded",
      percent: 12,
      root: "/work/beta",
      project: "beta",
    });
    const summary = summarizeSync(jobsOf(event(), failed));
    expect(summary?.failed).toBe(failed);
    expect(summary?.target).toBe(failed);
    expect(summary?.percent).toBe(40);
    expect(summary?.title.split("\n")).toEqual([
      "Contributors sync stopped · beta · git exploded",
      "Syncing contributors · alpha · Commits 3,200/12,000",
    ]);
  });

  it("shows a lone failure without a percent", () => {
    const summary = summarizeSync(
      jobsOf(event({ state: "error", error: null })),
    );
    expect(summary?.percent).toBeNull();
    expect(summary?.title).toBe(
      "Contributors sync stopped · alpha · The sync failed.",
    );
  });
});
