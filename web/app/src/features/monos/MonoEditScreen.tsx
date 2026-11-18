// F-78: edit a Mono at `/d/:desktopId/monos/:monoId/profile/edit`. Control devices only (read-only devices see why
// and no controls) and only when the desktop advertises `monoTools`. Everything goes through `mono.update`:
// the name saves 600 ms after typing stops (or when the page closes), mascot and colour save at once, projects
// are added and removed one by one (only projects this device may see are listed), and "Sessions it starts" is
// the model for sessions the Mono launches. The Mono's own model is `session.configure` on its conversation
// (the model controls). The chat colour is phone-local: kept in this browser, never sent to the desktop.
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ModelCatalog, ParamsOf } from "@monoptah/protocol";
import { catalogKey, useCatalog, useLinkMutation } from "@/data";
import { ModelControls } from "@/features/models/ModelControls";
import { ModelPicker, summarizeSelection } from "@/features/models/ModelPicker";
import { useModelCatalog } from "@/features/models/hooks";
import { modelEffortSetting, type ModelSelection, type ModelSelectionInput } from "@/features/models/modelSettings";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { safeHexColor } from "@/lib/color";
import { cn } from "@/lib/utils";
import { useAppearance } from "@/lib/appearance";
import { ACCENT_COLORS, ACCENT_LABELS } from "@/features/themes/swatches";
import { ACCENTS, type Accent } from "@/styles/themes";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { Notice } from "@/ui/Notice";
import { setMonoAccent, useMonoAccent } from "./monoAccent";
import { MONO_MASCOT_NAMES, MONO_NAME_MAX, MONO_SWATCHES } from "./monoLook";
import { MonoMascot } from "./MonoMascot";
import { BACK_LINK_CLASS, BackIcon, Group, MonoPage } from "./MonoPage";

export const NAME_AUTOSAVE_MS = 600;

type Update = ParamsOf<"mono.update">;

export function MonoEditScreen({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  const readOnly = useReadOnly();
  const catalog = useCatalog();
  const mono = catalog.data?.monos.find((candidate) => candidate.id === monoId);
  const monoTools = catalog.data?.monoTools === true;
  const back = (
    <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId }} aria-label="Back" className={BACK_LINK_CLASS}>
      <BackIcon />
    </Link>
  );
  return (
    <MonoPage testId="mono-edit" title="Edit Mono" back={back}>
      {readOnly ? (
        <Notice>Read only: this device can look at {mono?.name ?? "this Mono"} but cannot change it.</Notice>
      ) : !catalog.data ? (
        catalog.isError ? <Notice variant="error">Could not load the Mono.</Notice> : <DelayedSpinner label="Loading the Mono" className="m-6" />
      ) : !monoTools ? (
        <Notice>This desktop does not allow editing Monos from here.</Notice>
      ) : !mono ? (
        <Notice variant="error">This Mono is no longer there.</Notice>
      ) : (
        <Editor desktopId={desktopId} monoId={monoId} mono={mono} projects={catalog.data.projects} />
      )}
    </MonoPage>
  );
}

type CatalogMono = NonNullable<ReturnType<typeof useCatalog>["data"]>["monos"][number];
type CatalogProject = NonNullable<ReturnType<typeof useCatalog>["data"]>["projects"][number];

function Editor({ desktopId, monoId, mono, projects }: { desktopId: string; monoId: string; mono: CatalogMono; projects: CatalogProject[] }) {
  const queryClient = useQueryClient();
  const update = useLinkMutation("mono.update");
  const [error, setError] = useState<string | null>(null);
  const [look, setLook] = useState({ mascot: mono.mascot ?? "ghost", color: safeHexColor(mono.color, MONO_SWATCHES[0]) });
  const [assigned, setAssigned] = useState<readonly string[]>(mono.projectIds ?? []);

  const save = (change: Omit<Update, "monoId">, onFail?: () => void) => {
    setError(null);
    update.mutate(
      { monoId, ...change },
      {
        onSuccess: () => void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) }),
        onError: (failure) => {
          onFail?.();
          setError(failure.message || "Could not save.");
        },
      },
    );
  };

  // The name saves once typing pauses, not on every key, or as the page closes if that comes first.
  // A cleared name leaves the saved one.
  const [name, setName] = useState(mono.name);
  const saved = useRef(mono.name);
  const pending = useRef<string | null>(null);
  const flush = useRef(() => {});
  useEffect(() => {
    flush.current = () => {
      const next = pending.current;
      pending.current = null;
      if (next === null) return;
      saved.current = next;
      save({ name: next });
    };
  });
  useEffect(() => {
    const next = name.trim();
    if (!next || next === saved.current) {
      pending.current = null;
      return;
    }
    pending.current = next;
    const timer = setTimeout(() => flush.current(), NAME_AUTOSAVE_MS);
    return () => clearTimeout(timer);
  }, [name]);
  useEffect(() => () => flush.current(), []);

  const pickMascot = (mascot: string) => {
    if (mascot === look.mascot) return;
    const previous = look;
    setLook({ ...look, mascot });
    save({ mascot }, () => setLook(previous));
  };
  const pickColor = (color: string) => {
    if (color === look.color.toLowerCase()) return;
    const previous = look;
    setLook({ ...look, color });
    save({ color }, () => setLook(previous));
  };
  const toggleProject = (projectId: string, on: boolean) => {
    const previous = assigned;
    setAssigned(on ? [...assigned, projectId] : assigned.filter((id) => id !== projectId));
    save(on ? { addProject: projectId } : { removeProject: projectId }, () => setAssigned(previous));
  };

  return (
    <>
      <div className="flex justify-center py-2">
        <MonoMascot monoId={monoId} mascot={look.mascot} color={look.color} size={88} />
      </div>
      {error ? <Notice variant="error">{error}</Notice> : null}

      <Group title="Name">
        <Input aria-label="Name" value={name} maxLength={MONO_NAME_MAX} placeholder="Name" onChange={(event) => setName(event.target.value)} className="border-0 bg-transparent" />
      </Group>

      <Group title="Model">
        <div className="px-1 py-2">
          <ModelControls conversationId={`mono:${monoId}`} onError={setError} />
        </div>
      </Group>

      <SubsessionsGroup monoId={monoId} subsessions={mono.subsessions ?? null} onSave={(subsessions) => save({ subsessions })} />

      <Group title="Mascot">
        <div role="radiogroup" aria-label="Mascot" className="grid grid-cols-5 gap-2 p-1.5">
          {MONO_MASCOT_NAMES.map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={option === look.mascot}
              aria-label={option}
              onClick={() => pickMascot(option)}
              className={cn("grid aspect-square place-items-center rounded-card border-2 border-transparent hover:bg-selection", option === look.mascot && "border-[var(--accent-color)]")}
            >
              <MonoMascot monoId={monoId} mascot={option} color={look.color} size={40} />
            </button>
          ))}
        </div>
      </Group>

      <Group title="Color">
        <div role="radiogroup" aria-label="Color" className="flex flex-wrap gap-3 p-2.5">
          {MONO_SWATCHES.map((swatch) => {
            const selected = swatch === look.color.toLowerCase();
            return (
              <button
                key={swatch}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-label={swatch}
                onClick={() => pickColor(swatch)}
                className="grid size-11 place-items-center rounded-pill border-2 border-transparent pointer-fine:size-9"
                style={selected ? { borderColor: swatch } : undefined}
              >
                <span className="grid size-7 place-items-center rounded-pill text-white" style={{ background: swatch }}>
                  {selected ? <Icon name="check" size={14} /> : null}
                </span>
              </button>
            );
          })}
        </div>
      </Group>

      <ChatColor desktopId={desktopId} monoId={monoId} />

      <Group title="Projects">
        {projects.length === 0 ? (
          <p className="px-2 py-3 text-small text-muted-text">This device has no projects to assign.</p>
        ) : (
          <ul>
            {projects.map((project) => {
              const on = assigned.includes(project.id);
              return (
                <li key={project.id}>
                  <Checkbox label={<span className="truncate text-row">{project.name}</span>} checked={on} className="rounded-row px-2 hover:bg-selection" onChange={(event) => toggleProject(project.id, event.target.checked)} />
                </li>
              );
            })}
          </ul>
        )}
      </Group>
      <p className="px-1 text-meta text-muted-text">Without a project the Mono works from your home folder.</p>
    </>
  );
}

/** The model and effort for sessions the Mono starts without naming one (`subsessions`). */
function SubsessionsGroup({ monoId, subsessions, onSave }: { monoId: string; subsessions: NonNullable<CatalogMono["subsessions"]> | null; onSave: (value: NonNullable<Update["subsessions"]>) => void }) {
  const catalog = useModelCatalog();
  const [open, setOpen] = useState(false);
  const data: ModelCatalog | undefined = catalog.data;
  const value = useMemo<ModelSelectionInput | null>(() => {
    if (!data) return null;
    const harness = subsessions?.harness ?? data.current.harness;
    const model = subsessions?.model ?? data.current.model;
    const entry = data.models.find((candidate) => candidate.id === model);
    const effortId = modelEffortSetting(entry)?.id;
    return { harness, model, modelSettings: effortId && subsessions?.effort ? { [effortId]: subsessions.effort } : data.current.modelSettings, runtimeMode: data.current.runtimeMode };
  }, [data, subsessions]);
  const summary = data && value ? summarizeSelection(data, value) : null;
  const pick = (selection: ModelSelection) => {
    const entry = data?.models.find((candidate) => candidate.id === selection.model);
    const effortId = modelEffortSetting(entry)?.id;
    const effort = effortId ? selection.modelSettings[effortId] : undefined;
    onSave({ harness: selection.harness as NonNullable<Update["subsessions"]>["harness"], model: selection.model, ...(effort ? { effort } : {}) });
  };
  return (
    <>
      <Group title="Sessions it starts">
        <button
          type="button"
          disabled={!data}
          aria-label={summary ? `Sessions it starts: ${summary.name}${summary.effort ? `, ${summary.effort}` : ""}` : "Sessions it starts"}
          onClick={() => setOpen(true)}
          className="flex min-h-11 w-full items-center gap-3 rounded-row px-2 text-left hover:bg-selection disabled:opacity-50"
          data-mono={monoId}
        >
          {value ? <HarnessIcon harness={value.harness} className="size-4 shrink-0" /> : null}
          <span className="min-w-0 flex-1 truncate text-row">{summary?.name ?? "Choose a model"}</span>
          {summary?.effort ? <span className="text-meta text-muted-text">{summary.effort}</span> : null}
          <Icon name="chevronRight" size={16} className="text-muted-text" />
        </button>
      </Group>
      <p className="-mt-2 px-1 text-meta text-muted-text">The model and effort for sessions it starts without naming one.</p>
      {data && value ? <ModelPicker open={open} onOpenChange={setOpen} catalog={data} value={value} onChange={pick} showAccess={false} /> : null}
    </>
  );
}

/** The accent for this Mono's chat: phone-local, never sent to the desktop. */
function ChatColor({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  const accent = useMonoAccent(desktopId, monoId);
  const scheme = useAppearance((state) => state.scheme);
  return (
    <>
      <Group title="Chat color">
        <div role="radiogroup" aria-label="Chat color" className="flex flex-wrap gap-3 p-2.5">
          {ACCENTS.map((option: Accent) => {
            const selected = option === accent;
            const swatch = option === "theme" ? null : ACCENT_COLORS[option][scheme === "dark" ? 0 : 1];
            return (
              <button
                key={option}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-label={ACCENT_LABELS[option]}
                onClick={() => setMonoAccent(desktopId, monoId, option)}
                className="grid size-11 place-items-center rounded-pill border-2 border-transparent pointer-fine:size-9"
                style={selected ? { borderColor: swatch ?? "var(--content)" } : undefined}
              >
                <span className="grid size-7 place-items-center rounded-pill border border-stroke text-white" style={{ background: swatch ?? "var(--surface-raised)" }}>
                  {selected ? <Icon name="check" size={14} className={swatch ? undefined : "text-content"} /> : null}
                </span>
              </button>
            );
          })}
        </div>
      </Group>
      <p className="-mt-2 px-1 text-meta text-muted-text">Only on this device: it colours this chat and is not sent to your desktop.</p>
    </>
  );
}
