// U-06: the changes list, diff screen, git status and git actions against the real link wired to the mock
// desktop, whose project.changes, project.git and project.gitAction are replaced in-process (and restored).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { clearPanels } from "@/features/layout/panels";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetHighlighting } from "@/features/markdown/highlight/plugin";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { MAX_KEPT_DIFFS, clearKeptDiffs, keepDiff, keptDiff, keptDiffCount } from "./shownDiffs";
import { POLL_MS, barAction, safePrUrl, type ChangedFile, type GitStatus } from "./gitModel";

function stubViewport(width: number) {
  window.matchMedia = ((query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query);
    return { matches: min ? width >= Number(min[1]) : false, media: query, onchange: null, addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false };
  }) as unknown as typeof window.matchMedia;
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

type Line = ChangedFile["lines"][number];
const ctx = (n: number): Line => ({ kind: "context", text: `keep ${n}`, oldNumber: n, newNumber: n });
const manyContext = Array.from({ length: 30 }, (_, i) => ctx(i + 1));

function makeFile(relative: string, extra: Partial<ChangedFile> = {}): ChangedFile {
  return {
    relative,
    status: "modified",
    additions: 1,
    deletions: 1,
    lines: [
      { kind: "hunk", text: "@@ -1 +1 @@", oldNumber: null, newNumber: null },
      { kind: "del", text: "old line", oldNumber: 1, newNumber: null },
      { kind: "add", text: "new <b>line</b>", oldNumber: null, newNumber: 1 },
    ],
    ...extra,
  };
}

const handlers = FIXTURE_HANDLERS as unknown as Record<string, (p: never, c: never) => unknown>;
const originals = { changes: handlers["project.changes"], git: handlers["project.git"], action: handlers["project.gitAction"] };

let files: ChangedFile[];
let status: GitStatus;
let actions: Array<Record<string, unknown>>;
let changesCalls: number;
let actionImpl: (params: Record<string, unknown>) => unknown;

beforeEach(() => {
  changesCalls = 0;
  actions = [];
  files = [makeFile("src/a.ts", { staged: true, unstaged: false }), makeFile("src/b.ts"), makeFile("README.md", { status: "added", additions: 3, deletions: 0 })];
  status = { branch: "feature/x", files: 3, remote: true, upstream: true, defaultBranch: "main", ahead: 0, behind: 0, pr: null };
  actionImpl = (params) => {
    if (params.action === "generate") return { message: "feat: generated" };
    return { message: "ok" };
  };
  handlers["project.changes"] = (() => {
    changesCalls += 1;
    return { branch: status.branch, files: structuredClone(files) };
  }) as never;
  handlers["project.git"] = (() => structuredClone(status)) as never;
  handlers["project.gitAction"] = ((p: Record<string, unknown>) => {
    actions.push(p);
    return actionImpl(p);
  }) as never;
});

let previous: PersistAdapter;
beforeEach(() => {
  localStorage.clear();
  clearKeptDiffs();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  handlers["project.changes"] = originals.changes as never;
  handlers["project.git"] = originals.git as never;
  handlers["project.gitAction"] = originals.action as never;
  vi.useRealTimers();
  vi.restoreAllMocks();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  resetHighlighting();
  clearSlots();
  clearHooks();
  clearPanels();
  localStorage.clear();
});

const NO_PUSH = KNOWN_CAPABILITIES.filter((name) => name !== "gitPush") as string[];

function mount(path: string, { permission = "control", capabilities }: { permission?: "read" | "control"; capabilities?: string[] } = {}) {
  stubViewport(1280);
  connectMockDesktop({
    seed: (desktop) => {
      seedWorkspace(desktop);
      // The desktop toggle "Remote push in Git" is what lets sync, pr and pushing commits through.
      desktop.flags.remoteGitPush = !capabilities || capabilities.includes("gitPush");
    },
    permission,
    ...(capabilities ? { capabilities } : {}),
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/p/proj-alpha/${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

const list = () => mount("changes/");

describe("changes list (F-64, F-65)", () => {
  it("shows staged and unstaged sections with status letters and counts", async () => {
    list();
    const staged = await screen.findByRole("region", { name: "Staged Changes" });
    const changes = screen.getByRole("region", { name: "Changes" });
    expect(within(staged).getAllByTestId("change-row")).toHaveLength(1);
    expect(within(changes).getAllByTestId("change-row")).toHaveLength(2);
    const readme = within(changes).getByRole("button", { name: "Open diff of README.md" });
    expect(readme).toHaveTextContent("A");
    expect(readme).toHaveTextContent("+3");
    expect(within(staged).getByRole("button", { name: "Open diff of src/a.ts" })).toHaveTextContent("M");
  });

  it("stages a file at once and keeps it after the desktop answers", async () => {
    const user = userEvent.setup();
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    files = files.map((file) => (file.relative === "src/b.ts" ? { ...file, staged: true, unstaged: false } : file));
    await user.click(screen.getByRole("checkbox", { name: "Stage b.ts" }));
    expect(within(screen.getByRole("region", { name: "Staged Changes" })).getAllByTestId("change-row")).toHaveLength(2);
    await waitFor(() => expect(actions).toContainEqual(expect.objectContaining({ action: "stage", paths: ["src/b.ts"] })));
    expect(within(screen.getByRole("region", { name: "Staged Changes" })).getAllByTestId("change-row")).toHaveLength(2);
  });

  it("rolls the optimistic stage back and says why when the desktop refuses", async () => {
    const user = userEvent.setup();
    actionImpl = () => {
      throw new FixtureError("Index is locked");
    };
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("checkbox", { name: "Stage b.ts" }));
    expect(await screen.findByText(/Could not stage: Index is locked/)).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Staged Changes" })).getAllByTestId("change-row")).toHaveLength(1);
    expect(screen.getByRole("checkbox", { name: "Stage b.ts" })).toHaveAttribute("aria-checked", "false");
  });

  it("stages everything with the section button", async () => {
    const user = userEvent.setup();
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("button", { name: "Stage All" }));
    await waitFor(() => expect(actions).toContainEqual(expect.objectContaining({ action: "stageAll" })));
  });

  it("hides every control on a read-only device", async () => {
    mount("changes/", { permission: "read" });
    await screen.findByRole("region", { name: "Staged Changes" });
    expect(screen.getByText(/This device is read-only/)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /Stage All|Unstage All/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Commit/ })).toBeNull();
    expect(actions).toHaveLength(0);
  });

  it("refetches the list after 10 seconds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    const before = changesCalls;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(POLL_MS + 500);
    });
    await waitFor(() => expect(changesCalls).toBeGreaterThan(before));
  });

  it("polls the list and the git status every 10 seconds while visible", async () => {
    const { queryClient } = list();
    await screen.findByRole("region", { name: "Staged Changes" });
    expect(POLL_MS).toBe(10_000);
    await waitFor(() => {
      const intervals = queryClient
        .getQueryCache()
        .getAll()
        .filter((query) => query.queryKey[2] === "project.changes" || query.queryKey[2] === "project.git")
        .map((query) => query.observers[0]?.options.refetchInterval);
      expect(intervals).toEqual([POLL_MS, POLL_MS]);
    });
  });
});

describe("bottom action", () => {
  const base: GitStatus = { branch: "f", files: 0, remote: true, upstream: true, defaultBranch: "main", ahead: 0, behind: 0, pr: null };
  it("follows the branch state and never offers a push without the toggle", () => {
    const pick = (changed: number, staged: number, s: Partial<GitStatus>, canPush = true) => barAction({ changed, staged, status: { ...base, ...s }, canPush })?.label;
    expect(pick(2, 2, {})).toBe("Commit 2 files");
    expect(pick(1, 1, {})).toBe("Commit 1 file");
    expect(pick(2, 0, {})).toBe("Stage All");
    expect(pick(0, 0, { upstream: false })).toBe("Publish Branch");
    expect(pick(0, 0, { ahead: 1 })).toBe("Sync");
    expect(pick(0, 0, { pr: { number: 7, url: "https://x.test/7" } })).toBe("PR #7");
    expect(pick(0, 0, { aheadOfDefault: 2 })).toBe("Create PR");
    for (const s of [{ upstream: false }, { ahead: 1 }, { aheadOfDefault: 2 }, { pr: { number: 7, url: "https://x.test/7" } }]) {
      expect(pick(0, 0, s, false)).toBeUndefined();
    }
    expect(pick(2, 2, {}, false)).toBe("Commit 2 files");
  });
});

describe("commit (F-66)", () => {
  it("commits without push and without createPr", async () => {
    const user = userEvent.setup();
    mount("changes/", { capabilities: NO_PUSH });
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    expect(within(dialog).queryByRole("button", { name: "Commit and push" })).toBeNull();
    await user.type(within(dialog).getByRole("textbox", { name: "Commit message" }), "fix: thing");
    await user.click(within(dialog).getByRole("button", { name: "Commit" }));
    await waitFor(() => expect(actions.some((a) => a.action === "commit")).toBe(true));
    const sent = actions.find((a) => a.action === "commit") as Record<string, unknown>;
    expect(sent).toEqual({ projectId: "proj-alpha", action: "commit", message: "fix: thing" });
    expect("push" in sent).toBe(false);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Commit" })).toBeNull());
  });

  it("writes a message, and Stop writing drops the late answer", async () => {
    const user = userEvent.setup();
    let released = false;
    // The desktop answers {pending: true} while it writes; the client asks again under the same id.
    actionImpl = (params) => (params.action === "generate" ? (released ? { message: "feat: late" } : { pending: true }) : { message: "ok" });
    mount("changes/", { capabilities: NO_PUSH });
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await user.click(await within(dialog).findByRole("button", { name: "Stop writing" }));
    expect(within(dialog).getByRole("button", { name: "Write a message" })).toBeInTheDocument();
    released = true;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(within(dialog).getByRole("textbox", { name: "Commit message" })).toHaveValue("");
  });

  it("fills the message from a finished generation", async () => {
    const user = userEvent.setup();
    mount("changes/", { capabilities: NO_PUSH });
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await waitFor(() => expect(within(dialog).getByRole("textbox", { name: "Commit message" })).toHaveValue("feat: generated"));
  });
});

describe("push controls (H-06)", () => {
  beforeEach(() => {
    files = [];
    status = { ...status, files: 0, ahead: 2 };
  });

  it("are hidden while the desktop does not report Remote push in Git", async () => {
    mount("changes/", { capabilities: NO_PUSH });
    await screen.findByText("No changes");
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(screen.queryByRole("button", { name: /Sync|Publish Branch|Create PR|PR #/ })).toBeNull();
    expect(actions).toHaveLength(0);
  });

  it("show Sync and send nothing until the dialog naming the branch is confirmed", async () => {
    const user = userEvent.setup();
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Sync" }));
    const dialog = await screen.findByRole("alertdialog").catch(() => screen.findByRole("dialog"));
    expect(dialog).toHaveTextContent('"feature/x"');
    expect(actions).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Push" }));
    await waitFor(() => expect(actions).toContainEqual({ projectId: "proj-alpha", action: "sync" }));
  });

  it("asks a second time before pushing the default branch", async () => {
    const user = userEvent.setup();
    status = { ...status, branch: "main" };
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Sync" }));
    const first = await screen.findByRole("dialog");
    await user.click(within(first).getByRole("button", { name: "Push" }));
    expect(actions).toHaveLength(0);
    const second = await screen.findByRole("dialog");
    expect(second).toHaveTextContent('default branch "main"');
    await user.click(within(second).getByRole("button", { name: "Push to default branch" }));
    await waitFor(() => expect(actions).toContainEqual({ projectId: "proj-alpha", action: "sync" }));
  });

  it("commit and push sends push: true only after the confirm", async () => {
    const user = userEvent.setup();
    files = [makeFile("src/a.ts", { staged: true, unstaged: false })];
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    await user.type(within(dialog).getByRole("textbox", { name: "Commit message" }), "fix: it");
    await user.click(within(dialog).getByRole("button", { name: "Commit and push" }));
    expect(actions.filter((a) => a.action === "commit")).toHaveLength(0);
    const confirm = await screen.findByRole("dialog", { name: /Commit and push "feature\/x"/ });
    await user.click(within(confirm).getByRole("button", { name: "Push" }));
    await waitFor(() => expect(actions).toContainEqual({ projectId: "proj-alpha", action: "commit", message: "fix: it", push: true }));
  });

  it("opens only an https pull request URL after Create PR", async () => {
    const user = userEvent.setup();
    status = { ...status, ahead: 0, aheadOfDefault: 2 };
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    actionImpl = () => ({ url: "https://github.com/o/r/pull/9" });
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Create PR" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Create PR" }));
    await user.click(await screen.findByRole("button", { name: "Open pull request" }));
    expect(open).toHaveBeenCalledWith("https://github.com/o/r/pull/9", "_blank", "noopener,noreferrer");
  });

  it("refuses a javascript: pull request URL", async () => {
    const user = userEvent.setup();
    status = { ...status, ahead: 0, aheadOfDefault: 2 };
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    actionImpl = () => ({ url: "javascript:alert(1)" });
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Create PR" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Create PR" }));
    expect(await screen.findByText(/not a safe https address/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open pull request" })).toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("is never offered to a read-only device even with the toggle on", async () => {
    mount("changes/", { permission: "read" });
    await screen.findByText("No changes");
    expect(screen.queryByRole("button", { name: "Sync" })).toBeNull();
  });
});

describe("safePrUrl", () => {
  it("accepts https only", () => {
    expect(safePrUrl("https://github.com/o/r/pull/1")).toBe("https://github.com/o/r/pull/1");
    for (const bad of ["http://github.com/x", "javascript:alert(1)", "data:text/html,x", "file:///etc/passwd", "//evil.test", "", "not a url", undefined, 5]) {
      expect(safePrUrl(bad)).toBeNull();
    }
  });
});

describe("diff screen (F-68)", () => {
  it("renders lines as text with a gutter and never as markup", async () => {
    mount("changes/src/b.ts");
    const view = await screen.findByTestId("diff-view");
    await waitFor(() => expect(view.textContent).toContain("new <b>line</b>"));
    expect(view.querySelector("b")).toBeNull();
    const rows = within(view).getAllByRole("row");
    expect(rows.some((row) => row.dataset["kind"] === "hunk")).toBe(true);
    expect(rows.find((row) => row.dataset["kind"] === "add")).toHaveTextContent("1");
  });

  it("folds long unchanged stretches and expands them step by step and all at once", async () => {
    const user = userEvent.setup();
    files = [
      makeFile("src/long.ts", {
        lines: [{ kind: "hunk", text: "@@", oldNumber: null, newNumber: null }, ...manyContext, { kind: "add", text: "tail change", oldNumber: null, newNumber: 31 }],
      }),
    ];
    mount("changes/src/long.ts");
    const fold = await screen.findByTestId("diff-fold");
    // 30 context lines, the last 3 stay visible next to the change: 27 hidden.
    expect(within(fold).getByText("27 unchanged lines")).toBeInTheDocument();
    await user.click(within(fold).getByRole("button", { name: "Show 20 more" }));
    expect(within(fold).getByText("7 unchanged lines")).toBeInTheDocument();
    await user.click(within(fold).getByRole("button", { name: "Show all 7" }));
    expect(within(fold).queryByText(/unchanged line/)).toBeNull();
    expect(within(fold).getAllByRole("row")).toHaveLength(27);
  });

  it("toggles wrapping", async () => {
    const user = userEvent.setup();
    mount("changes/src/b.ts");
    const toggle = await screen.findByRole("button", { name: "Wrap lines" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });

  it("opens from the list and keeps showing a diff that left the list", async () => {
    const user = userEvent.setup();
    list();
    await user.click(await screen.findByRole("button", { name: "Open diff of src/b.ts" }));
    await screen.findByTestId("diff-view");
    files = [];
    await act(async () => {
      fireEvent.focus(window);
    });
    expect(keptDiff("proj-alpha", "src/b.ts")).toBeDefined();
    expect(screen.getByTestId("diff-view")).toBeInTheDocument();
  });

  it("says so when a file has no changes and was never opened", async () => {
    mount("changes/src/none.ts");
    expect(await screen.findByText("No longer changed")).toBeInTheDocument();
  });

  it("keeps at most the last 4 diffs in memory", () => {
    for (let i = 0; i < 7; i += 1) keepDiff("p", makeFile(`f${i}.ts`));
    expect(keptDiffCount()).toBe(MAX_KEPT_DIFFS);
    expect(keptDiff("p", "f0.ts")).toBeUndefined();
    expect(keptDiff("p", "f6.ts")).toBeDefined();
  });
});

describe("git status (F-67)", () => {
  it("shows branch, ahead and behind, upstream and the PR", async () => {
    status = { ...status, ahead: 2, behind: 1, pr: { number: 12, url: "https://github.com/o/r/pull/12" } };
    mount("git");
    const view = await screen.findByTestId("git-status");
    expect(view).toHaveTextContent("feature/x");
    expect(view).toHaveTextContent("2 ahead, 1 behind");
    expect(view).toHaveTextContent("Tracking a remote branch");
    expect(within(view).getByRole("button", { name: "PR #12" })).toBeInTheDocument();
  });
});
