// `/d/:desktopId/p/:projectId/new` (F-30, F-31): "What should we work on?" and the composer that starts a session
// with `session.start`. Model, effort, account and access come from the shared `ModelControls`; where the work
// happens comes from the workspace menu (the checkout, a worktree, or a new worktree); where it runs comes from
// the `new-session-target` slot (null means this desktop). The `send-guard` hook runs before the start. After a
// start the page opens the session, or goes back to the project when "Open after starting" is off (F-20).
import { Link, useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";
import { SessionStartTargetSchema, type ParamsOf } from "@monoptah/protocol";
import { catalogKey, useCatalog, useLinkMutation } from "@/data";
import { BACKGROUND_CONTAINER_CLASS, EmptySessionBackground } from "@/features/backgrounds";
import { assertFitsRequest, toWire } from "@/features/composer/attachments";
import { useRequestControl } from "@/features/composer/useRequestControl";
import { Composer } from "@/features/composer/Composer";
import { useLinkReady } from "@/features/composer/SessionComposer";
import { useKeyboardInset } from "@/features/composer/useKeyboardInset";
import { useSendGuard } from "@/features/composer/useSendGuard";
import { ModelControls } from "@/features/models/ModelControls";
import { startingSelection, useDefaultModels } from "@/features/models/defaultModel";
import { useModelCatalog } from "@/features/models/hooks";
import { toEmitted, type ModelSelection } from "@/features/models/modelSettings";
import { buildStartModelParams } from "@/features/models/params";
import { Icon } from "@/ui/Icon";
import { Switch } from "@/ui/switch";
import { toast } from "@/ui/sonner";
import { useOpenAfterStarting } from "./openAfterStarting";
import { ProjectMascot } from "./ProjectMascot";
import { NewSessionTargetSlot } from "./slots";
import { accentFor } from "./WorkspaceNav";
import { WorkspaceMenu } from "./WorkspaceMenu";
import type { Worktree } from "./sessionListFilter";
import { useReadOnly } from "./WorkspaceFrame";
import { useRememberPlace } from "./lastPlace";
import { clearNewSessionDraft, readNewSessionDraft, writeNewSessionDraft } from "./newSessionDraft";

export const EMPTY_HEADLINE = "What should we work on?";

/** The `kind` of a target value the slot reported, for the guard's context. */
function targetKind(target: unknown): { kind: string } | null {
  if (target && typeof target === "object" && "kind" in target && typeof (target as { kind: unknown }).kind === "string") {
    return { kind: (target as { kind: string }).kind };
  }
  return null;
}

/**
 * `embedded` is the wide layout of F-18: from 1024 px the page is the chat pane, beside the session-list sidebar, so it
 * has no header (the sidebar owns the only h1 and the way back) and fills the pane edge to edge, with no card.
 */
export function NewSessionScreen({ desktopId, projectId, embedded = false }: { desktopId: string; projectId: string; embedded?: boolean }) {
  const keyboardInset = useKeyboardInset();
  const requestControl = useRequestControl(desktopId);
  useRememberPlace();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const readOnly = useReadOnly();
  const ready = useLinkReady();
  const guard = useSendGuard();
  const start = useLinkMutation("session.start");
  const [openAfter, setOpenAfter] = useOpenAfterStarting();
  const catalog = useCatalog().data;
  const project = catalog?.projects.find((candidate) => candidate.id === projectId);
  const models = useModelCatalog({ projectId });
  const { defaults, loaded } = useDefaultModels(desktopId);

  // The text and the model pick live in a per-project store too, so they outlive this page being unmounted (F-18).
  const [initialDraft] = useState(() => readNewSessionDraft(desktopId, projectId));
  const [picked, setPickedState] = useState<ModelSelection | null>(initialDraft.picked);
  const setPicked = (next: ModelSelection | null) => {
    setPickedState(next);
    writeNewSessionDraft(desktopId, projectId, { picked: next });
  };
  const rememberText = useCallback((text: string) => writeNewSessionDraft(desktopId, projectId, { text }), [desktopId, projectId]);
  const [worktree, setWorktree] = useState<Worktree | null>(null);
  const [newWorktree, setNewWorktree] = useState(false);
  const [target, setTarget] = useState<unknown>(null);
  const [configError, setConfigError] = useState<string | null>(null);

  // What the controls show and what `session.start` sends: the pick, else the U-07 default for this desktop.
  const starting = useMemo(() => (models.data && loaded ? startingSelection(models.data, defaults) : undefined), [models.data, loaded, defaults]);
  const selection = picked ?? (starting ? toEmitted(starting) : undefined);
  const shown = picked ? { ...picked, runtimeMode: picked.runtimeMode ?? starting?.runtimeMode ?? "supervised" } : starting;
  const defaultModelId = defaults.harness ? defaults.models[defaults.harness]?.model : undefined;

  const submit = async (text: string, attachments: Parameters<typeof toWire>[0]) => {
    let validTarget: ReturnType<typeof SessionStartTargetSchema.parse> | undefined;
    if (target !== null && target !== undefined) {
      const parsed = SessionStartTargetSchema.safeParse(target);
      if (!parsed.success) throw new Error("This target cannot be used from the web.");
      validTarget = parsed.data;
    }
    assertFitsRequest(text, attachments);
    // There is no session yet: the guard gets an empty session id and the target the user picked.
    await guard.check({ desktopId, sessionId: "", target: targetKind(target) });
    const wire = toWire(attachments);
    const result = await start.mutateAsync({
      projectId,
      text,
      ...(wire.length > 0 ? { attachments: wire } : {}),
      ...(buildStartModelParams(selection) as Partial<ParamsOf<"session.start">>),
      ...(worktree ? { worktreePath: worktree.path } : {}),
      ...(newWorktree ? { workspaceMode: "worktree" as const } : {}),
      ...(validTarget ? { target: validTarget } : {}),
    });
    clearNewSessionDraft(desktopId, projectId);
    void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
    void queryClient.invalidateQueries({ queryKey: ["link", desktopId, "sessions.list"] });
    const sessionId = result.sessionId;
    if (openAfter && sessionId) {
      void navigate({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId } });
    } else {
      toast("Session started");
      void navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId } });
    }
  };

  const topRow = (
    <>
      {project ? (
        <WorkspaceMenu
          desktopId={desktopId}
          projectId={projectId}
          projectPath={project.path}
          checkoutBranch={project.branch}
          value={worktree}
          onChange={(next) => {
            setWorktree(next);
            setNewWorktree(false);
          }}
          onNewWorktree={() => {
            setNewWorktree(true);
            setWorktree(null);
          }}
          readOnly={readOnly}
        />
      ) : null}
      {newWorktree ? (
        <button
          type="button"
          aria-label="New worktree, remove"
          onClick={() => setNewWorktree(false)}
          className="inline-flex min-h-9 items-center gap-1.5 rounded-pill bg-selection px-3 text-label text-brand"
        >
          <Icon name="branch" size={14} />
          New worktree
          <Icon name="close" size={12} />
        </button>
      ) : null}
      <NewSessionTargetSlot desktopId={desktopId} projectId={projectId} value={target} onChange={(next) => setTarget(next ?? null)} />
    </>
  );

  return (
    <div
      data-testid="new-session-screen"
      data-embedded={embedded ? "true" : undefined}
      className={`${BACKGROUND_CONTAINER_CLASS} flex h-full min-h-0 flex-1 flex-col ${embedded ? "overflow-hidden" : ""}`}
    >
      <EmptySessionBackground projectId={projectId} />
      {embedded ? null : (
      <header className="mx-auto flex w-full max-w-[768px] shrink-0 items-center gap-2 px-2 py-2 min-[24rem]:px-5">
        <Link
          to="/d/$desktopId/p/$projectId"
          params={{ desktopId, projectId }}
          aria-label="Back"
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection"
        >
          <Icon name="chevronDown" size={20} className="rotate-90" />
        </Link>
        <h1 className="min-w-0 flex-1 truncate text-h font-semibold">New session</h1>
      </header>
      )}

      <div className="mx-auto flex min-h-0 w-full max-w-[768px] flex-1 flex-col items-center justify-center gap-3 px-5 text-center">
        {project ? <ProjectMascot project={project.path} name={project.mascot} color={accentFor(project.name, project.color)} size={64} className="opacity-40" /> : null}
        <h2 className="text-title font-semibold">{EMPTY_HEADLINE}</h2>
        {project ? <p className="text-body text-muted-text">{project.name}</p> : null}
        {catalog && !project ? <p className="text-body text-muted-text">This project is not on this device.</p> : null}
        {/* F-20: mobile keeps this in its settings; the web puts it where the session starts. */}
        <label className="mt-2 inline-flex min-h-11 items-center gap-3 text-label text-muted-text">
          Open after starting
          <Switch checked={openAfter} onCheckedChange={setOpenAfter} aria-label="Open after starting" />
        </label>
      </div>

      <div className={`mx-auto w-full max-w-[768px] shrink-0 px-2 min-[24rem]:px-5 ${keyboardInset > 0 ? "" : "pb-[env(safe-area-inset-bottom)]"}`}>
        <Composer
          autoFocus={!embedded}
          initialText={initialDraft.text}
          onTextChange={rememberText}
          onSubmit={submit}
          readOnly={readOnly}
          requestControl={requestControl}
          offline={!ready}
          topRow={topRow}
          toolbar={
            <ModelControls
              projectId={projectId}
              value={shown}
              onChange={setPicked}
              defaultModelId={defaultModelId}
              disabled={readOnly}
              onError={setConfigError}
            />
          }
        />
        {configError ? (
          <p role="alert" className="-mt-2 px-4 pb-2 text-meta text-deletions">
            {configError}
          </p>
        ) : null}
      </div>
      {guard.dialog}
    </div>
  );
}
