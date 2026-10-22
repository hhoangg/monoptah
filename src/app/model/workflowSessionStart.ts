import {
  startAppSession,
  type AppSessionListing,
  type StartHost,
} from "../../features/agent-app/model/agentApp";
import type { Mono } from "../../features/monos/model/mono";
import type { MonoLaunchRegistry } from "../../features/monos/model/monoLaunches";
import type { ControlOutcome } from "../../features/orchestration/model/orchestration";
import { sameProjectPath } from "../../features/projects/model/recents";
import type { QuickLaunch } from "../../features/quick-composer/model/quickComposer";
import type { Session } from "../../features/sessions/model/session";
import type { Worktrees } from "../../features/source-control/model/worktrees";
import type { SessionRequest } from "../hooks/workflowRunner";

export type WorkflowSessionDeps = {
  findMono(monoId: string): Mono | undefined;
  /** The Mono's own chat, loaded if it is not open. */
  openSession(sessionId: string): Promise<Session | null>;
  /** Accepts a launch like `sessions.start` does and reports when its first turn settles. */
  launch(
    launch: QuickLaunch,
    id: string,
    onSettled: (outcome: ControlOutcome) => void,
  ): Promise<void>;
  registry: MonoLaunchRegistry;
  isMonoSession(sessionId: string): boolean;
  openSessions(): Session[];
  storedSessions(cwd: string): Promise<StoredSummary[]>;
  storedSession(sessionId: string): Promise<Session | null>;
  worktrees(cwd: string): Promise<Worktrees>;
};

type StoredSummary = Pick<Session, "id" | "title" | "harness" | "model"> & {
  archived?: boolean;
  draft?: boolean;
};

const listing = (
  session: StoredSummary,
  busy: boolean,
  hasDraft: boolean,
): AppSessionListing => ({
  id: session.id,
  title: session.title,
  harness: session.harness,
  model: session.model,
  busy,
  hasDraft,
  archived: !!session.archived,
});

/**
 * Starts a workflow node's session through the same code `sessions.start` runs, as the Mono:
 * the project check, worktrees, nesting under another session and filing in a folder all
 * behave as they do for a session the Mono starts itself. Differences: no completion report
 * goes to the Mono (the run watches the session itself), the session runs with full access
 * like a habit run, and it always shows in the sidebar so the run can be followed there.
 */
export async function startWorkflowSession(
  deps: WorkflowSessionDeps,
  monoId: string,
  req: SessionRequest,
): Promise<{
  sessionId: string;
  folderId?: string;
  settled: Promise<ControlOutcome>;
}> {
  const mono = deps.findMono(monoId);
  const chat = mono?.sessionId ? await deps.openSession(mono.sessionId) : null;
  if (!mono || !chat) throw new Error("The Mono's chat is not available");

  // Nesting is decoration: the app forgets who started what when it restarts, and a child
  // cannot be a parent, so a parent that does not qualify just means a top-level session.
  const known = req.parentSessionId
    ? deps.registry.get(req.parentSessionId)
    : undefined;
  const parentSessionId =
    known && known.monoId === monoId && !known.parented
      ? req.parentSessionId
      : undefined;

  let onSettled!: (outcome: ControlOutcome) => void;
  const settled = new Promise<ControlOutcome>((resolve) => {
    onSettled = resolve;
  });
  const host: StartHost = {
    start: (launch, id) => deps.launch(launch, id, onSettled),
    sessions: async (cwd) => {
      const byId = new Map<string, AppSessionListing>();
      for (const stored of await deps.storedSessions(cwd))
        byId.set(stored.id, listing(stored, false, !!stored.draft));
      for (const open of deps.openSessions())
        if (sameProjectPath(open.cwd, cwd) && !open.orchestrationLeadId)
          byId.set(open.id, listing(open, !!open.busy, false));
      return [...byId.values()];
    },
    session: async (id) =>
      deps.openSessions().find((entry) => entry.id === id) ??
      (await deps.storedSession(id)),
    worktrees: (cwd) => deps.worktrees(cwd),
    isMono: (id) => deps.isMonoSession(id),
    isHabitRun: () => false,
    launches: deps.registry,
    monoOf: (id) =>
      id === chat.id
        ? {
            id: mono.id,
            projects: mono.projects,
            sessionId: chat.id,
            color: mono.color,
            showStartedSessionsInSidebar: true,
          }
        : undefined,
  };
  const requestId = `wf${crypto.randomUUID().replace(/-/g, "")}`;
  const started = (await startAppSession(
    chat,
    requestId,
    {
      project: req.project,
      prompt: req.prompt,
      ...(req.harness ? { harness: req.harness } : {}),
      ...(req.model ? { model: req.model } : {}),
      ...(req.effort ? { effort: req.effort } : {}),
      runtimeMode: "full-access",
      workspaceMode: req.workspaceMode,
      ...(req.worktreeCwd ? { worktreeCwd: req.worktreeCwd } : {}),
      ...(parentSessionId ? { parentSessionId } : {}),
      ...(req.folderId ? { folderId: req.folderId } : {}),
      ...(req.folderName ? { folderName: req.folderName } : {}),
      notifyOnComplete: false,
    },
    host,
  )) as { id: string; folderId?: string };
  return { sessionId: started.id, folderId: started.folderId, settled };
}
