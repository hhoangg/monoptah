// U-06: the changes list, diff screen, git status and git actions against the real link wired to the mock
// desktop, whose project.changes, project.git and project.gitAction are replaced in-process (and restored).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { clearPanels, openPanel, registerPanel } from "@/features/layout/panels";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetHighlighting } from "@/features/markdown/highlight/plugin";
import { LinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
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

// The route files import the panel registrations for their side effects: start every test from an empty registry,
// so `vitest -t` (which skips the earlier tests' cleanup) finds nothing registered twice.
beforeEach(() => {
  clearPanels();
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
    const readme = within(changes).getByRole("button", { name: /^Open diff of README\.md/ });
    expect(readme).toHaveTextContent("A");
    expect(readme).toHaveTextContent("+3");
    expect(within(staged).getByRole("button", { name: /^Open diff of src\/a\.ts/ })).toHaveTextContent("M");
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
    // The confirm sheet of external links comes first (W13-7): nothing opens until "Open link".
    expect(open).not.toHaveBeenCalled();
    expect(screen.getByTestId("external-link-host")).toHaveTextContent("github.com");
    await user.click(screen.getByRole("button", { name: "Open link" }));
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
    for (const bad of ["http://github.com/x", "javascript:alert(1)", "data:text/html,x", "file:///etc/passwd", "//evil.test", "", "not a url", undefined, 5, "https://user:pw@github.com/o/r/pull/1", "https://github.com/o/r/pull/1\u202E", "https://\u0430pple.com/pull/1", "https://github.com/o r/1"]) {
      expect(safePrUrl(bad), String(bad)).toBeNull();
    }
  });
});

describe("diff screen (F-68)", () => {
  it("renders lines as text with a gutter and never as markup", async () => {
    mount("changes/src/b.ts");
    const view = await screen.findByTestId("diff-view");
    await waitFor(() => expect(view.textContent).toContain("new <b>line</b>"));
    expect(view.querySelector("b")).toBeNull();
    const rows = within(view).getAllByRole("listitem");
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
    expect(within(fold).getAllByRole("listitem")).toHaveLength(27);
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
    const { queryClient } = list();
    await user.click(await screen.findByRole("button", { name: /^Open diff of src\/b\.ts/ }));
    await screen.findByTestId("diff-view");
    // The desktop no longer lists the file, and the list is really read again (not a focus event that refetches nothing).
    files = [];
    const before = changesCalls;
    await act(async () => {
      await queryClient.refetchQueries({ predicate: (query) => query.queryKey[2] === "project.changes" });
    });
    expect(changesCalls).toBeGreaterThan(before);
    const cached = queryClient.getQueryCache().getAll().find((query) => query.queryKey[2] === "project.changes");
    expect((cached?.state.data as { files: unknown[] } | undefined)?.files).toEqual([]);
    expect(keptDiff(DESKTOP_ID, "proj-alpha", "src/b.ts")).toBeDefined();
    expect(screen.getByTestId("diff-view")).toHaveTextContent("src/b.ts");
    expect(screen.queryByText("No longer changed")).toBeNull();
  });

  it("says so when a file has no changes and was never opened", async () => {
    mount("changes/src/none.ts");
    expect(await screen.findByText("No longer changed")).toBeInTheDocument();
  });

  it("keeps at most the last 4 diffs in memory", () => {
    for (let i = 0; i < 7; i += 1) keepDiff("d", "p", makeFile(`f${i}.ts`));
    expect(keptDiffCount()).toBe(MAX_KEPT_DIFFS);
    expect(keptDiff("d", "p", "f0.ts")).toBeUndefined();
    expect(keptDiff("d", "p", "f6.ts")).toBeDefined();
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

  it("names the git remote", async () => {
    status = { ...status, remoteName: "origin" };
    mount("git");
    expect(await within(await screen.findByTestId("git-status")).findByText("origin")).toBeInTheDocument();
  });
});

/** Holds every `project.gitAction` answer until the test releases it, and records what was sent and under which id. */
function holdGitActions() {
  const sent: Array<{ params: Record<string, unknown>; id: string | undefined }> = [];
  const gates: Array<() => void> = [];
  const original = LinkConnection.prototype.request;
  vi.spyOn(LinkConnection.prototype, "request").mockImplementation(function (this: LinkConnection, method: string, params?: unknown, options?: { id?: string }) {
    const result = (original as (...args: unknown[]) => Promise<unknown>).call(this, method, params, options);
    if (method !== "project.gitAction") return result as never;
    sent.push({ params: params as Record<string, unknown>, id: options?.id });
    return new Promise((resolve, reject) => gates.push(() => void result.then(resolve, reject))) as never;
  } as never);
  return { sent, release: () => gates.shift()?.() };
}

/** Records the command id of each `project.gitAction` without holding the answer. */
function recordGitIds() {
  const sent: Array<{ params: Record<string, unknown>; id: string | undefined }> = [];
  const original = LinkConnection.prototype.request;
  vi.spyOn(LinkConnection.prototype, "request").mockImplementation(function (this: LinkConnection, method: string, params?: unknown, options?: { id?: string }) {
    if (method === "project.gitAction") sent.push({ params: params as Record<string, unknown>, id: options?.id });
    return (original as (...args: unknown[]) => unknown).call(this, method, params, options) as never;
  } as never);
  return sent;
}

describe("quick toggles (race)", () => {
  it("sends the toggles one after the other and keeps the row where the user left it", async () => {
    const user = userEvent.setup();
    // The desktop applies each call to its state, so the list read after a call shows it.
    actionImpl = (params) => {
      if (params.action === "stage" || params.action === "unstage") {
        const paths = params.paths as string[];
        files = files.map((file) => (paths.includes(file.relative) ? { ...file, staged: params.action === "stage", unstaged: params.action !== "stage" } : file));
      }
      return { message: "ok" };
    };
    const held = holdGitActions();
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("checkbox", { name: "Stage b.ts" }));
    // Toggled again before the desktop answered the first call.
    await user.click(await screen.findByRole("checkbox", { name: "Unstage b.ts" }));
    await waitFor(() => expect(held.sent).toHaveLength(1));
    expect(held.sent[0]?.params).toMatchObject({ action: "stage", paths: ["src/b.ts"] });
    expect(screen.getByRole("checkbox", { name: "Stage b.ts" })).toHaveAttribute("aria-checked", "false");
    // The first answer arrives: the row must not flip back to staged while the second call is still to be sent.
    held.release();
    await waitFor(() => expect(held.sent).toHaveLength(2));
    expect(held.sent[1]?.params).toMatchObject({ action: "unstage", paths: ["src/b.ts"] });
    expect(screen.getByRole("checkbox", { name: "Stage b.ts" })).toHaveAttribute("aria-checked", "false");
    held.release();
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "Stage b.ts" })).toHaveAttribute("aria-checked", "false"));
    expect(files.find((file) => file.relative === "src/b.ts")?.staged).toBe(false);
  });

  it("disables the per-file toggles while Stage All runs", async () => {
    const user = userEvent.setup();
    const held = holdGitActions();
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("button", { name: "Stage All" }));
    await waitFor(() => expect(held.sent).toHaveLength(1));
    for (const box of screen.getAllByRole("checkbox")) expect(box).toBeDisabled();
    held.release();
    await waitFor(() => expect(screen.getAllByRole("checkbox").every((box) => !(box as HTMLButtonElement).disabled)).toBe(true));
  });
});

describe("one queue per project", () => {
  it("holds a generate behind a stage call that is still in flight", async () => {
    const user = userEvent.setup();
    actionImpl = (params) => (params.action === "generate" ? { message: "feat: ok" } : { message: "ok" });
    const held = holdGitActions();
    mount("changes/", { capabilities: NO_PUSH });
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("checkbox", { name: "Stage b.ts" }));
    await waitFor(() => expect(held.sent).toHaveLength(1));
    // The bar and the dialog use their own runner: their call must still wait for the stage call.
    await user.click(await screen.findByRole("button", { name: /^Commit \d+ files?$/ }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(held.sent).toHaveLength(1);
    held.release();
    await waitFor(() => expect(held.sent).toHaveLength(2));
    expect(held.sent[1]?.params).toMatchObject({ action: "generate" });
    held.release();
  });

  it("removes its abort listener from the lifetime signal when a call settles", async () => {
    const user = userEvent.setup();
    actionImpl = () => ({ message: "ok" });
    const added: unknown[] = [];
    const removed: unknown[] = [];
    const add = AbortSignal.prototype.addEventListener;
    const remove = AbortSignal.prototype.removeEventListener;
    vi.spyOn(AbortSignal.prototype, "addEventListener").mockImplementation(function (this: AbortSignal, type: string, listener: never, options?: never) {
      if (type === "abort" && (listener as { name?: string }).name === "abort") added.push(listener);
      return add.call(this, type, listener, options);
    } as never);
    vi.spyOn(AbortSignal.prototype, "removeEventListener").mockImplementation(function (this: AbortSignal, type: string, listener: never, options?: never) {
      if (type === "abort" && (listener as { name?: string }).name === "abort") removed.push(listener);
      return remove.call(this, type, listener, options);
    } as never);
    list();
    await screen.findByRole("region", { name: "Staged Changes" });
    await user.click(screen.getByRole("button", { name: "Stage All" }));
    await waitFor(() => expect(added.length).toBeGreaterThan(0));
    await waitFor(() => expect(removed).toEqual(added));
  });
});

describe("generate ids and polling", () => {
  it("re-sends a pending generation under the same id, also after Stop writing, and stops polling on Stop", async () => {
    const user = userEvent.setup();
    actionImpl = (params) => (params.action === "generate" ? { pending: true } : { message: "ok" });
    const sent = recordGitIds();
    mount("changes/", { capabilities: NO_PUSH });
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    const generates = () => sent.filter((call) => call.params["action"] === "generate");
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await waitFor(() => expect(generates().length).toBeGreaterThanOrEqual(2), { timeout: 3000 });
    expect(new Set(generates().map((call) => call.id)).size).toBe(1);
    const firstId = generates()[0]?.id;
    await user.click(await within(dialog).findByRole("button", { name: "Stop writing" }));
    // Let a request that was already out settle, then the count must stay put: the polling loop ended.
    await new Promise((resolve) => setTimeout(resolve, 700));
    const settled = generates().length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(generates()).toHaveLength(settled);
    // Writing again is the same command to the desktop.
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await waitFor(() => expect(generates().length).toBeGreaterThan(settled));
    expect(generates().at(-1)?.id).toBe(firstId);
  }, 15_000);

  it("stops polling when the dialog closes", async () => {
    const user = userEvent.setup();
    actionImpl = (params) => (params.action === "generate" ? { pending: true } : { message: "ok" });
    const sent = recordGitIds();
    mount("changes/", { capabilities: NO_PUSH });
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const dialog = await screen.findByRole("dialog", { name: "Commit" });
    await user.click(within(dialog).getByRole("button", { name: "Write a message" }));
    await waitFor(() => expect(sent.filter((call) => call.params["action"] === "generate").length).toBeGreaterThanOrEqual(1));
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await new Promise((resolve) => setTimeout(resolve, 700));
    const settled = sent.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(sent).toHaveLength(settled);
  }, 15_000);
});

describe("next steps with a dirty tree (F-66)", () => {
  it("offers Sync beside Commit, and confirms it like the bottom action", async () => {
    const user = userEvent.setup();
    status = { ...status, ahead: 2 };
    mount("changes/");
    await screen.findByRole("button", { name: "Commit 1 file" });
    await user.click(await screen.findByRole("button", { name: "Sync" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent('"feature/x"');
    expect(actions).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Push" }));
    await waitFor(() => expect(actions).toContainEqual({ projectId: "proj-alpha", action: "sync" }));
  });

  it("offers Publish Branch in the commit dialog and asks twice on the default branch", async () => {
    const user = userEvent.setup();
    status = { ...status, branch: "main", upstream: false };
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "Commit 1 file" }));
    const commit = await screen.findByRole("dialog", { name: "Commit" });
    await user.click(within(commit).getByRole("button", { name: "Publish Branch" }));
    const first = await screen.findByRole("dialog", { name: /Publish "main"/ });
    await user.click(within(first).getByRole("button", { name: "Push" }));
    expect(actions).toHaveLength(0);
    const second = await screen.findByRole("dialog", { name: /default branch "main"/ });
    await user.click(within(second).getByRole("button", { name: "Push to default branch" }));
    await waitFor(() => expect(actions).toContainEqual({ projectId: "proj-alpha", action: "sync" }));
  });

  it("shows View PR for an open pull request even with changes, and never Create PR while the tree is dirty", async () => {
    const user = userEvent.setup();
    status = { ...status, aheadOfDefault: 3, pr: { number: 7, url: "https://github.com/o/r/pull/7" } };
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    mount("changes/");
    await user.click(await screen.findByRole("button", { name: "View PR #7" }));
    expect(open).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Open link" }));
    expect(open).toHaveBeenCalledWith("https://github.com/o/r/pull/7", "_blank", "noopener,noreferrer");
    expect(screen.queryByRole("button", { name: "Create PR" })).toBeNull();
  });

  it("is on the Git screen too, and hidden without Remote push in Git", async () => {
    status = { ...status, upstream: false };
    mount("git");
    expect(await within(await screen.findByTestId("git-status")).findByRole("button", { name: "Publish Branch" })).toBeInTheDocument();
  });

  it("is hidden while the desktop does not report Remote push in Git", async () => {
    status = { ...status, upstream: false, ahead: 1 };
    mount("changes/", { capabilities: NO_PUSH });
    await screen.findByRole("button", { name: "Commit 1 file" });
    expect(screen.queryByRole("button", { name: /Publish Branch|Sync|View PR/ })).toBeNull();
  });
});

describe("session-capabilities predicate", () => {
  it("hides the changes screen, its data and the cross-link to Git when changes is refused", async () => {
    registerSessionCapabilities((capability) => capability !== "changes");
    mount("changes/");
    expect(await screen.findByText("Changes are not available for this project.")).toBeInTheDocument();
    expect(changesCalls).toBe(0);
    expect(screen.queryByTestId("changes-view")).toBeNull();
  });

  it("hides the cross-link to Git on the changes screen when git is refused", async () => {
    registerSessionCapabilities((capability) => capability !== "git");
    mount("changes/");
    await screen.findByTestId("changes-view");
    expect(screen.queryByRole("link", { name: "Git status" })).toBeNull();
  });

  it("refuses the git screen, and its link to Changes when changes is refused", async () => {
    registerSessionCapabilities((capability) => capability !== "git");
    mount("git");
    expect(await screen.findByText("Git is not available for this project.")).toBeInTheDocument();
    expect(screen.queryByTestId("git-status")).toBeNull();
  });

  it("hides the Changes link on the git screen when changes is refused", async () => {
    registerSessionCapabilities((capability) => capability !== "changes");
    mount("git");
    await screen.findByTestId("git-status");
    expect(screen.queryByRole("link", { name: "Changes" })).toBeNull();
  });

  it("cannot open the panels the predicate refuses", () => {
    for (const kind of ["files", "changes", "git"] as const) registerPanel(kind, { title: kind, component: () => null });
    registerSessionCapabilities((capability) => capability === "changes");
    const target = { desktopId: DESKTOP_ID, projectId: "proj-alpha" };
    expect(openPanel("changes", target, { target: null, desktopCapabilities: [] })).toBe(true);
    expect(openPanel("git", target, { target: null, desktopCapabilities: [] })).toBe(false);
    expect(openPanel("files", target, { target: null, desktopCapabilities: [] })).toBe(false);
  });
});
