// The first screen of a new terminal (a tab or the phone drawer): which project to open the shell in. Only the projects
// shared with this device are listed (the catalog of the link). After "Open terminal" the browser asks for the passkey
// (controller.begin), and then the shell opens.
import { isSafeTerminalSubdir, TERMINAL_SUBDIR_MAX } from "@monoptah/protocol";
import { useMemo, useState, type FormEvent } from "react";
import { useCatalog } from "@/data";
import { useCapability } from "@/version";
import { useLinkState } from "@/link";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Field } from "@/ui/Field";
import { Notice } from "@/ui/Notice";
import { NOT_ALLOWED_NOTICE } from "./copy";
import type { ChosenTarget } from "./session";

export function TerminalChooser({ error, onOpen }: { error: string | null; onOpen: (target: ChosenTarget) => void }) {
  const allowed = useCapability("terminal");
  const permission = useLinkState((state) => state.permission);
  if (!allowed) {
    return (
      <div data-testid="terminal-disabled" className="flex flex-col gap-3 p-4">
        <Notice variant="info" title="The terminal is off for this device">
          {permission === "read" ? "A terminal needs control access to this desktop." : NOT_ALLOWED_NOTICE}
        </Notice>
      </div>
    );
  }
  return <Form error={error} onOpen={onOpen} />;
}

function Form({ error, onOpen }: { error: string | null; onOpen: (target: ChosenTarget) => void }) {
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
    onOpen({ projectId: chosen.id, ...(folder === "" ? {} : { subdir: folder }), label: folder === "" ? chosen.name : `${chosen.name}/${folder}` });
  }

  if (catalog.isPending) return <DelayedSpinner label="Loading projects" className="m-6" />;
  if (projects.length === 0) {
    return (
      <div className="p-4">
        <Notice variant="info">No project of this desktop is shared with this device.</Notice>
      </div>
    );
  }
  return (
    <form data-testid="terminal-chooser" onSubmit={submit} className="flex w-full max-w-md flex-col gap-4 p-4">
      <p className="text-small text-muted-text">Opens a real shell on your desktop, as the user who runs Monoptah. Every open asks for your passkey first.</p>
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
