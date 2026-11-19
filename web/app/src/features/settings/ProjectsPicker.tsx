import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { DeviceSelection } from "@monoptah/protocol";
import { Button } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { DesktopLinkError, type DesktopLink } from "./desktopLink";
import { selectionQuery, workspaceQuery } from "./desktopQueries";

interface Item {
  id: string;
  name: string;
  detail?: string;
}

export const REFUSED_COPY = "The desktop only lets this browser see fewer projects and Monos, never more. To add some back, change it on the desktop.";
const SAVE_ERROR = "Could not save the selection. Try again.";

/** True when `id` is in the granted list (null means everything is granted). */
function granted(list: string[] | null, id: string): boolean {
  return list === null || list.includes(id);
}

function failureCopy(error: unknown): string {
  if (error instanceof DesktopLinkError) {
    if (error.code === "forbidden") return `${REFUSED_COPY} (Desktop said: ${error.message})`;
    if (error.code === "read_only") return "This browser can only read from the desktop, so it cannot change the selection.";
    if (error.code === "desktop_offline") return "The desktop is offline. Try again when it is online.";
  }
  return SAVE_ERROR;
}

export interface ProjectsPickerProps {
  desktopId: string;
  link: DesktopLink;
  open: boolean;
  /** Called when the overlay closes by any route: Save, "Choose later" / Cancel, Escape, outside tap. */
  onOpenChange: (open: boolean) => void;
  /** `first-run` offers "Choose later"; `settings` offers "Cancel". Both keep the granted selection and make no set call. */
  mode: "first-run" | "settings";
}

/**
 * F-10 "Projects on this device". Reads `device.selection` (get) and `workspace.list`, shows a checkbox
 * per project and Mono, and Save calls `device.selection` (set) with the changed lists. The desktop may
 * only be narrowed from here: a refusal (for example when an item outside the granted selection is
 * checked) is shown as the desktop answered it. Overlay: vaul Drawer under 640px, Dialog from 640px.
 */
export function ProjectsPicker({ desktopId, link, open, onOpenChange, mode }: ProjectsPickerProps) {
  const queryClient = useQueryClient();
  const selection = useQuery({ ...selectionQuery(desktopId, link), enabled: open });
  const workspace = useQuery({ ...workspaceQuery(desktopId, link), enabled: open });
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const current = selection.data;
  const catalog = workspace.data;
  const projects: Item[] = (catalog?.projects ?? []).map((project) => ({ id: project.id, name: project.name, ...(project.path ? { detail: project.path } : {}) }));
  const monos: Item[] = (catalog?.monos ?? []).map((mono) => ({ id: mono.id, name: mono.name }));

  const isChecked = (kind: "p" | "m", item: Item): boolean => {
    const override = overrides[`${kind}:${item.id}`];
    if (override !== undefined) return override;
    return current ? granted(kind === "p" ? current.projects : current.monos, item.id) : false;
  };

  /** The list to send for one kind, or null when it is unchanged. */
  function changedList(kind: "p" | "m", items: Item[], grantedList: string[] | null): string[] | null {
    const changed = items.some((item) => isChecked(kind, item) !== granted(grantedList, item.id));
    if (!changed) return null;
    const checked = items.filter((item) => isChecked(kind, item)).map((item) => item.id);
    const unseen = (grantedList ?? []).filter((id) => !items.some((item) => item.id === id));
    return [...checked, ...unseen];
  }

  const nextProjects = current ? changedList("p", projects, current.projects) : null;
  const nextMonos = current ? changedList("m", monos, current.monos) : null;
  const dirty = nextProjects !== null || nextMonos !== null;

  function close(next: boolean): void {
    if (saving) return;
    if (!next) {
      setOverrides({});
      setError(null);
    }
    onOpenChange(next);
  }

  async function save(): Promise<void> {
    if (!dirty || saving) return;
    setSaving(true);
    setError(null);
    try {
      const result: DeviceSelection = await link.call("device.selection", {
        ...(nextProjects ? { projects: nextProjects } : {}),
        ...(nextMonos ? { monos: nextMonos } : {}),
      });
      queryClient.setQueryData(selectionQuery(desktopId, link).queryKey, result);
      await queryClient.invalidateQueries({ queryKey: workspaceQuery(desktopId, link).queryKey });
      setSaving(false);
      close(false);
    } catch (caught) {
      setSaving(false);
      setError(failureCopy(caught));
    }
  }

  async function refresh(): Promise<void> {
    setRefreshing(true);
    await Promise.allSettled([selection.refetch(), workspace.refetch()]);
    setRefreshing(false);
  }

  const failed = selection.isError || workspace.isError;
  const ready = current !== undefined && catalog !== undefined;

  return (
    <ResponsiveOverlay
      open={open}
      onOpenChange={close}
      title="Projects on this device"
      description="Choose which projects and Monos this browser can see. You can only narrow what the desktop allowed, never widen it."
      footer={
        <>
          <Button type="button" variant="ghost" disabled={saving} onClick={() => close(false)}>
            {mode === "first-run" ? "Choose later" : "Cancel"}
          </Button>
          <Button type="button" disabled={!ready || !dirty || saving} aria-busy={saving} onClick={() => void save()}>
            Save
            {saving ? <DelayedSpinner label="Saving" /> : null}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {error ? <Notice variant="error">{error}</Notice> : null}
        {failed && !ready ? <Notice variant="error">Could not read the selection from the desktop.</Notice> : null}
        {!ready && !failed ? (
          <div className="flex min-h-13 items-center justify-center">
            <DelayedSpinner label="Loading projects" />
          </div>
        ) : null}
        {ready ? (
          <>
            <PickerGroup title="Projects" empty="No projects on this desktop yet." items={projects} checked={(item) => isChecked("p", item)} onChange={(item, value) => setOverrides((all) => ({ ...all, [`p:${item.id}`]: value }))} />
            <PickerGroup title="Monos" empty="No Monos on this desktop yet." items={monos} checked={(item) => isChecked("m", item)} onChange={(item, value) => setOverrides((all) => ({ ...all, [`m:${item.id}`]: value }))} />
          </>
        ) : null}
        <Button type="button" variant="outline" size="sm" className="self-start" disabled={refreshing || saving} aria-busy={refreshing} onClick={() => void refresh()}>
          <Icon name="refresh" size={16} className={refreshing ? "animate-spin" : undefined} />
          Refresh desktop
        </Button>
      </div>
    </ResponsiveOverlay>
  );
}

function PickerGroup({ title, empty, items, checked, onChange }: { title: string; empty: string; items: Item[]; checked: (item: Item) => boolean; onChange: (item: Item, value: boolean) => void }) {
  return (
    <fieldset className="flex min-w-0 flex-col">
      <legend className="mb-1 px-1 text-label font-medium text-muted-text">{title}</legend>
      {items.length === 0 ? <p className="px-1 py-2 text-small text-muted-text">{empty}</p> : null}
      {items.map((item) => (
        <Checkbox
          key={item.id}
          checked={checked(item)}
          onChange={(event) => onChange(item, event.target.checked)}
          label={
            <span className="block min-w-0">
              <span className="block truncate">{item.name}</span>
              {item.detail ? <span className="block truncate text-meta text-muted-text">{item.detail}</span> : null}
            </span>
          }
        />
      ))}
    </fieldset>
  );
}
