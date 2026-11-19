import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { DeviceSelection, SelectionItem } from "@monoptah/protocol";
import { ApiError } from "@/features/accounts/api";
import { ReauthCancelledError } from "@/features/accounts/queries";
import { requestSelectionTicket } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";
import { Button } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { DesktopLinkError, type DesktopLink } from "./desktopLink";
import { selectionQuery, workspaceQuery } from "./desktopQueries";

/** A row of the list. Unshared rows carry no id and no path: the name is all the desktop tells this browser. */
interface Item {
  /** Stable key within one kind: the id for a shared item or a legacy row, the opaque ref otherwise. */
  key: string;
  /** Opaque handle the desktop resolves when the item is added; absent on a legacy (older desktop) row. */
  ref?: string;
  /** Present only for items this device already has. */
  id?: string;
  name: string;
  shared: boolean;
  detail?: string;
}

type Kind = "p" | "m";

/** Shown only for an older desktop that still refuses every widening. */
export const REFUSED_COPY = "The desktop only lets this browser see fewer projects and Monos, never more. To add some back, change it on the desktop.";
export const READ_ONLY_COPY = "Only a browser with Control access can add projects. You can still remove some.";
const SAVE_ERROR = "Could not save the selection. Try again.";

/** True when `id` is in the granted list (null means everything is granted). */
function granted(list: string[] | null, id: string): boolean {
  return list === null || list.includes(id);
}

function failureCopy(error: unknown, legacy: boolean): string {
  if (error instanceof ReauthCancelledError) return "Passkey confirmation was cancelled. Nothing was changed.";
  if (error instanceof ApiError) {
    if (error.code === "rate_limited") return "Too many attempts to add projects. Wait a while and try again.";
    if (error.code === "forbidden") return "This browser does not have Control access to this desktop, so it cannot add projects.";
    if (error.code === "fresh_required") return "Confirm with your passkey, then try again.";
    return SAVE_ERROR;
  }
  if (error instanceof DesktopLinkError) {
    switch (error.code) {
      case "forbidden":
        return legacy ? `${REFUSED_COPY} (Desktop said: ${error.message})` : "The desktop did not allow this change.";
      case "read_only":
        return READ_ONLY_COPY;
      case "fresh_required":
        return "The desktop could not confirm your passkey. Try again, and confirm with your passkey when asked.";
      case "rate_limited":
        return "Too many attempts to add projects on this desktop. Wait a while and try again.";
      case "revoked":
        return "This browser no longer has access to this desktop. Ask the owner to share it again.";
      case "not_found":
        return "The desktop's list changed. Refresh and try again.";
      case "desktop_offline":
        return "The desktop is offline. Try again when it is online.";
    }
  }
  return SAVE_ERROR;
}

/** What one kind (projects or Monos) sends, and whether sending it widens. */
interface KindChange {
  /** `device.selection` params for this kind; null when nothing changed. */
  params: { list: string[] | null; add: string[] } | null;
  widens: boolean;
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
 * F-10 "Projects on this device". Reads `device.selection` (get) and shows every project and Mono of the desktop
 * with its state, "Shared" or "Not shared". Unchecking removes at once on Save. Checking a "Not shared" item, or
 * turning on "All ..., including new ones", widens what this browser can see: that needs Control access and a
 * fresh passkey confirmation, proven to the desktop with a single-use ticket. A read-only browser can only remove.
 * An older desktop without `available` keeps the narrow-only list built from `workspace.list`.
 * Overlay: vaul Drawer under 640px, Dialog from 640px.
 */
export function ProjectsPicker({ desktopId, link, open, onOpenChange, mode }: ProjectsPickerProps) {
  const queryClient = useQueryClient();
  const device = useThisDevice();
  const selection = useQuery({ ...selectionQuery(desktopId, link), enabled: open });
  const workspace = useQuery({ ...workspaceQuery(desktopId, link), enabled: open });
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  const [allOverrides, setAllOverrides] = useState<{ p?: boolean; m?: boolean }>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const current = selection.data;
  const catalog = workspace.data;
  const available = current?.available;
  const legacy = current !== undefined && available === undefined;
  const canWiden = !legacy && current?.canWiden === true;

  const detailOf = new Map((catalog?.projects ?? []).map((project) => [project.id, project.path]));
  const projects: Item[] = available
    ? available.projects.map((item) => fromAvailable(item, detailOf))
    : (catalog?.projects ?? []).map((project) => ({ key: project.id, id: project.id, name: project.name, shared: true, ...(project.path ? { detail: project.path } : {}) }));
  const monos: Item[] = available ? available.monos.map((item) => fromAvailable(item)) : (catalog?.monos ?? []).map((mono) => ({ key: mono.id, id: mono.id, name: mono.name, shared: true }));

  const grantedList = (kind: Kind): string[] | null => (current ? (kind === "p" ? current.projects : current.monos) : []);
  /** Rows the user has unchecked (only meaningful in the modern list). */
  const rowChecked = (kind: Kind, item: Item): boolean => overrides[`${kind}:${item.key}`] ?? item.shared;
  /** "All, including new ones": the switch if the user used it, else the granted state until a row is unchecked. */
  const isAll = (kind: Kind, items: Item[]): boolean => allOverrides[kind] ?? (grantedList(kind) === null && items.every((item) => rowChecked(kind, item)));
  const isChecked = (kind: Kind, item: Item): boolean => {
    if (legacy) return overrides[`${kind}:${item.key}`] ?? (current ? granted(grantedList(kind), item.key) : false);
    return allOverrides[kind] === true || rowChecked(kind, item);
  };

  /** The change of one kind against the granted list, in the shape `device.selection` takes. */
  function changeOf(kind: Kind, items: Item[]): KindChange {
    const now = grantedList(kind);
    if (!current) return { params: null, widens: false };
    if (legacy) {
      const changed = items.some((item) => isChecked(kind, item) !== granted(now, item.key));
      if (!changed) return { params: null, widens: false };
      const checked = items.filter((item) => isChecked(kind, item)).map((item) => item.key);
      const unseen = (now ?? []).filter((id) => !items.some((item) => item.key === id));
      return { params: { list: [...checked, ...unseen], add: [] }, widens: false };
    }
    if (isAll(kind, items)) return now === null ? { params: null, widens: false } : { params: { list: null, add: [] }, widens: true };
    const keptShared = items.filter((item) => item.shared && isChecked(kind, item)).map((item) => item.id ?? item.key);
    // Granted ids the desktop no longer lists (a closed or unpublished project) stay granted, as in the legacy branch.
    const unseen = (now ?? []).filter((id) => !items.some((item) => item.id === id));
    const keep = [...keptShared, ...unseen];
    const add = items.filter((item) => !item.shared && isChecked(kind, item) && item.ref !== undefined).map((item) => item.ref as string);
    const removed = items.some((item) => item.shared && !isChecked(kind, item));
    // From "all" to a list (turned off) is a change even when every row is still checked.
    if (now !== null && !removed && add.length === 0) return { params: null, widens: false };
    return { params: { list: keep, add }, widens: add.length > 0 };
  }

  const projectChange = changeOf("p", projects);
  const monoChange = changeOf("m", monos);
  const dirty = projectChange.params !== null || monoChange.params !== null;
  const widens = projectChange.widens || monoChange.widens;

  function close(next: boolean): void {
    if (saving) return;
    if (!next) {
      setOverrides({});
      setAllOverrides({});
      setError(null);
    }
    onOpenChange(next);
  }

  async function save(): Promise<void> {
    if (!dirty || saving || !current) return;
    setSaving(true);
    setError(null);
    try {
      let ticket: string | undefined;
      if (widens) {
        if (!device?.deviceId) throw new Error("This browser is not registered with your account yet.");
        // Opens the passkey prompt when the session is not fresh. Never send a widening without the ticket.
        ticket = await requestSelectionTicket(desktopId, device.deviceId);
      }
      const result: DeviceSelection = await link.call("device.selection", {
        ...(projectChange.params ? { projects: projectChange.params.list, ...(projectChange.params.add.length > 0 ? { addProjects: projectChange.params.add } : {}) } : {}),
        ...(monoChange.params ? { monos: monoChange.params.list, ...(monoChange.params.add.length > 0 ? { addMonos: monoChange.params.add } : {}) } : {}),
        ...(ticket ? { ticket } : {}),
      });
      queryClient.setQueryData(selectionQuery(desktopId, link).queryKey, result);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: workspaceQuery(desktopId, link).queryKey }),
        // The link's own catalog (rail, sessions) follows what this browser can now see.
        queryClient.invalidateQueries({ queryKey: ["link", desktopId, "workspace.list"] }),
        queryClient.invalidateQueries({ queryKey: ["link", desktopId, "sessions.list"] }),
      ]);
      setSaving(false);
      close(false);
    } catch (caught) {
      setSaving(false);
      setError(failureCopy(caught, legacy));
    }
  }

  async function refresh(): Promise<void> {
    setRefreshing(true);
    await Promise.allSettled([selection.refetch(), workspace.refetch()]);
    setRefreshing(false);
  }

  const failed = selection.isError || workspace.isError;
  const ready = current !== undefined && (catalog !== undefined || available !== undefined);
  const description = legacy
    ? "Choose which projects and Monos this browser can see. You can only narrow what the desktop allowed, never widen it."
    : canWiden
      ? "Choose which projects and Monos this browser can see. Adding one you do not share yet asks you to confirm with your passkey, and the desktop tells its owner."
      : "Choose which projects and Monos this browser can see. This browser can only remove some, not add.";

  return (
    <ResponsiveOverlay
      open={open}
      onOpenChange={close}
      title="Projects on this device"
      description={description}
      footer={
        <>
          <Button type="button" variant="ghost" disabled={saving} onClick={() => close(false)}>
            {mode === "first-run" ? "Choose later" : "Cancel"}
          </Button>
          <Button type="button" disabled={!ready || !dirty || saving} aria-busy={saving} onClick={() => void save()}>
            {widens ? "Confirm and save" : "Save"}
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
        {ready && !legacy && !canWiden ? <Notice variant="info">{READ_ONLY_COPY}</Notice> : null}
        {ready ? (
          <>
            <PickerGroup
              title="Projects"
              empty="No projects on this desktop yet."
              items={projects}
              showState={!legacy}
              all={legacy ? undefined : { label: "All projects, including new ones", checked: isAll("p", projects), disabled: !canWiden && grantedList("p") !== null, onChange: (value) => setAllOverrides((all) => ({ ...all, p: value })) }}
              checked={(item) => isChecked("p", item)}
              disabled={(item) => !legacy && (allOverrides.p === true || (!item.shared && !canWiden))}
              onChange={(item, value) => setOverrides((all) => ({ ...all, [`p:${item.key}`]: value }))}
            />
            <PickerGroup
              title="Monos"
              empty="No Monos on this desktop yet."
              items={monos}
              showState={!legacy}
              all={legacy ? undefined : { label: "All Monos, including new ones", checked: isAll("m", monos), disabled: !canWiden && grantedList("m") !== null, onChange: (value) => setAllOverrides((all) => ({ ...all, m: value })) }}
              checked={(item) => isChecked("m", item)}
              disabled={(item) => !legacy && (allOverrides.m === true || (!item.shared && !canWiden))}
              onChange={(item, value) => setOverrides((all) => ({ ...all, [`m:${item.key}`]: value }))}
            />
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

/** A row from `available`. A path is shown only for an item the device already has (from `workspace.list`). */
function fromAvailable(item: SelectionItem, details?: Map<string, string | undefined>): Item {
  const path = item.shared && item.id !== undefined ? details?.get(item.id) : undefined;
  return { key: item.shared && item.id !== undefined ? item.id : item.ref, ref: item.ref, ...(item.id !== undefined && item.shared ? { id: item.id } : {}), name: item.name, shared: item.shared, ...(path ? { detail: path } : {}) };
}

interface AllToggle {
  label: string;
  checked: boolean;
  disabled: boolean;
  onChange: (value: boolean) => void;
}

function PickerGroup({
  title,
  empty,
  items,
  showState,
  all,
  checked,
  disabled,
  onChange,
}: {
  title: string;
  empty: string;
  items: Item[];
  showState: boolean;
  all?: AllToggle;
  checked: (item: Item) => boolean;
  disabled: (item: Item) => boolean;
  onChange: (item: Item, value: boolean) => void;
}) {
  return (
    <fieldset className="flex min-w-0 flex-col">
      <legend className="mb-1 px-1 text-label font-medium text-muted-text">{title}</legend>
      {all ? <Checkbox checked={all.checked} disabled={all.disabled} onChange={(event) => all.onChange(event.target.checked)} label={<span className="block truncate font-medium">{all.label}</span>} /> : null}
      {items.length === 0 ? <p className="px-1 py-2 text-small text-muted-text">{empty}</p> : null}
      {items.map((item) => (
        <Checkbox
          key={item.key}
          checked={checked(item)}
          disabled={disabled(item)}
          onChange={(event) => onChange(item, event.target.checked)}
          label={
            <span className="block min-w-0">
              <span className="block truncate">{item.name}</span>
              {showState ? <span className="block text-meta text-muted-text">{item.shared ? "Shared" : "Not shared"}</span> : null}
              {item.detail ? <span className="block truncate text-meta text-muted-text">{item.detail}</span> : null}
            </span>
          }
        />
      ))}
    </fieldset>
  );
}
