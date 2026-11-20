// /d/:desktopId/terminal: choose a project (and optionally a folder), enter the code the desktop shows, then work in the
// shell. The page owns one `TerminalController` for as long as it is mounted. Leaving the page does not kill the shell: it
// stays for a reattach (the page finds it again through the resume entry) until it is idle, closed or the desktop ends it.
import { isSafeTerminalSubdir, TERMINAL_SUBDIR_MAX } from "@monoptah/protocol";
import { useEffect, useMemo, useState, useSyncExternalStore, type FormEvent } from "react";
import { useCatalog } from "@/data";
import { useCapability } from "@/version";
import { useLinkConnection, useLinkDesktopId, useLinkState } from "@/link";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Field } from "@/ui/Field";
import { Notice } from "@/ui/Notice";
import { CodeDialog } from "./CodeDialog";
import { NOT_ALLOWED_NOTICE } from "./copy";
import { TerminalController, type OpenTarget } from "./controller";
import { linkPortOf } from "./port";
import { sessionResumeStore } from "./resume";
import { TerminalView } from "./TerminalView";

/** One controller and resume store per desktop, with the reattach after a reload wired in. */
function useController(desktopId: string): TerminalController {
  const connection = useLinkConnection();
  const { controller, store } = useMemo(() => {
    const resume = sessionResumeStore(desktopId);
    return { controller: new TerminalController({ link: linkPortOf(connection), store: resume }), store: resume };
  }, [connection, desktopId]);
  useEffect(() => {
    // A shell a reload left behind (inside its grace window) is picked up again.
    // (Also after a dev double mount, which stops the loops of the first one: `resume` starts them again.)
    const saved = store.load();
    const phase = controller.getSnapshot().phase;
    if (saved && (phase === "idle" || phase === "reconnecting")) controller.resume(saved);
    // A reload or a closed tab: keep the offset the shell can be reattached from.
    const flush = (): void => controller.flushResume();
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      controller.dispose();
    };
  }, [controller, store]);
  return controller;
}

export function TerminalPage() {
  const desktopId = useLinkDesktopId();
  const controller = useController(desktopId);
  return <TerminalFlow key={desktopId} controller={controller} desktopId={desktopId} />;
}

function TerminalFlow({ controller, desktopId }: { controller: TerminalController; desktopId: string }) {
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const allowed = useCapability("terminal");
  const permission = useLinkState((state) => state.permission);
  const [target, setTarget] = useState<OpenTarget | null>(null);
  const [label, setLabel] = useState("");

  const { phase } = snapshot;
  // The view shows from the first click until the flow is back at the chooser; a resumed or running shell shows it too.
  // An open that failed (phase "failed") is back at the chooser, with the error shown there.
  const chooser = phase === "failed" || (phase === "idle" && target === null);

  if (chooser && !allowed) {
    return (
      <div data-testid="terminal-disabled" className="flex flex-col gap-3 py-4">
        <Notice variant="info" title="The terminal is off for this device">
          {permission === "read" ? "A terminal needs control access to this desktop." : NOT_ALLOWED_NOTICE}
        </Notice>
      </div>
    );
  }

  if (chooser) {
    return (
      <Chooser
        desktopId={desktopId}
        error={snapshot.error}
        onOpen={(next, name) => {
          controller.reset();
          setLabel(name);
          setTarget(next);
        }}
      />
    );
  }

  return (
    <>
      <TerminalView
        controller={controller}
        onScreen={() => {
          if (target && controller.getSnapshot().phase === "idle") void controller.begin(target);
        }}
        onLeave={() => {
          controller.reset();
          setTarget(null);
        }}
      />
      <CodeDialog
        snapshot={snapshot}
        where={label || "the project"}
        onSubmit={(code) => void controller.submitCode(code)}
        onNewCode={() => void controller.restart()}
        onCancel={() => {
          controller.cancel();
          setTarget(null);
        }}
      />
    </>
  );
}

function Chooser({ desktopId, error, onOpen }: { desktopId: string; error: string | null; onOpen: (target: OpenTarget, projectName: string) => void }) {
  const catalog = useCatalog();
  const projects = useMemo(() => catalog.data?.projects ?? [], [catalog.data]);
  const [projectId, setProjectId] = useState("");
  const [subdir, setSubdir] = useState("");
  const chosen = projects.find((project) => project.id === projectId) ?? projects[0];
  const folder = subdir.trim();
  const folderError = folder !== "" && !isSafeTerminalSubdir(folder) ? "Use a folder inside the project, written relative to it, without .." : null;

  function submit(event: FormEvent): void {
    event.preventDefault();
    if (!chosen || folderError) return;
    onOpen({ projectId: chosen.id, ...(folder === "" ? {} : { subdir: folder }) }, folder === "" ? chosen.name : `${chosen.name}/${folder}`);
  }

  if (catalog.isPending) return <DelayedSpinner label="Loading projects" className="m-6" />;
  if (projects.length === 0) {
    return (
      <Notice variant="info">No project of this desktop is shared with this device.</Notice>
    );
  }
  return (
    <form data-testid={`terminal-chooser-${desktopId}`} onSubmit={submit} className="flex max-w-md flex-col gap-4 py-2">
      <p className="text-small text-muted-text">Opens a real shell on your desktop, as the user who runs Monoptah. Every open needs a one-time code that the desktop shows on its own screen.</p>
      {error ? <Notice variant="error">{error}</Notice> : null}
      <div className="flex flex-col gap-1.5">
        <label htmlFor="terminal-project" className="text-label font-medium">
          Project
        </label>
        <select
          id="terminal-project"
          data-testid="terminal-project"
          value={chosen?.id ?? ""}
          onChange={(event) => setProjectId(event.target.value)}
          className="min-h-11 w-full min-w-0 rounded-row border border-stroke bg-fill px-3 py-2 text-row text-content pointer-fine:min-h-9"
        >
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
      </div>
      <Field
        label="Folder (optional)"
        data-testid="terminal-subdir"
        value={subdir}
        onChange={(event) => setSubdir(event.target.value)}
        maxLength={TERMINAL_SUBDIR_MAX}
        placeholder="src/app"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        hint="Relative to the project folder."
        error={folderError}
      />
      <Button type="submit" size="lg" disabled={!chosen || folderError !== null} data-testid="terminal-open">
        Open terminal
      </Button>
    </form>
  );
}
