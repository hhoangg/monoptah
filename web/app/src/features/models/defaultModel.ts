// The default model of one desktop (U-07, F-56), after mobile/src/features/settings/defaultModel.ts:
// the provider a new chat starts with, each provider's model (remembered as it is picked so switching
// back finds it) and the access it starts with. Stored per desktop under `defaultModel:<desktopId>`
// in the persisted cache adapter (encrypted IndexedDB once W-12 registers it; the second key segment is
// the desktop id, so "Remove this desktop" clears it). Access is sendable modes only.
import { useCallback, useEffect, useState } from "react";
import type { ModelCatalog, SendableRuntimeMode } from "@monoptah/protocol";
import { getPersistAdapter } from "@/data";
import { sendableAccess } from "./access";
import { currentSelection, mergeModelSettings, parseStringRecord, type ModelSelectionInput } from "./modelSettings";

export interface SavedModel {
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  /** The name it is shown by when the catalog is not loaded. */
  name: string;
}

export interface DefaultModels {
  /** The provider new chats start with; null leaves it to the desktop. */
  harness: string | null;
  models: Record<string, SavedModel>;
  /** The access new chats start with; null leaves it to the desktop. Never `full-access`. */
  runtimeMode: SendableRuntimeMode | null;
}

export const EMPTY_DEFAULTS: DefaultModels = { harness: null, models: {}, runtimeMode: null };

export const defaultModelKey = (desktopId: string): string => `defaultModel:${desktopId}`;

/** A damaged or foreign record is read as "never set"; only strings survive, `full-access` is dropped. */
export function parseDefaultModels(raw: unknown): DefaultModels {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return EMPTY_DEFAULTS;
  const record = raw as Record<string, unknown>;
  const models: Record<string, SavedModel> = {};
  if (record.models && typeof record.models === "object" && !Array.isArray(record.models)) {
    for (const [harness, entry] of Object.entries(record.models)) {
      if (!entry || typeof entry !== "object") continue;
      const saved = entry as Record<string, unknown>;
      if (typeof saved.model !== "string") continue;
      models[harness] = {
        harness,
        model: saved.model,
        modelSettings: parseStringRecord(saved.modelSettings),
        name: typeof saved.name === "string" ? saved.name : saved.model,
      };
    }
  }
  return {
    harness: typeof record.harness === "string" ? record.harness : null,
    models,
    runtimeMode: sendableAccess(typeof record.runtimeMode === "string" ? record.runtimeMode : undefined),
  };
}

export async function loadDefaultModels(desktopId: string): Promise<DefaultModels> {
  try {
    return parseDefaultModels(await getPersistAdapter().get(defaultModelKey(desktopId)));
  } catch {
    return EMPTY_DEFAULTS;
  }
}

const listeners = new Set<(desktopId: string) => void>();

export async function saveDefaultModels(desktopId: string, next: DefaultModels): Promise<void> {
  await getPersistAdapter().set(defaultModelKey(desktopId), parseDefaultModels(next));
  for (const listener of listeners) listener(desktopId);
}

/** The defaults of a desktop and a setter that saves them. `loaded` is false until the first read. */
export function useDefaultModels(desktopId: string) {
  const [state, setState] = useState<{ desktopId: string; defaults: DefaultModels } | null>(null);
  useEffect(() => {
    let live = true;
    const read = () => {
      void loadDefaultModels(desktopId).then((defaults) => {
        if (live) setState({ desktopId, defaults });
      });
    };
    read();
    const onSaved = (saved: string) => saved === desktopId && read();
    listeners.add(onSaved);
    return () => {
      live = false;
      listeners.delete(onSaved);
    };
  }, [desktopId]);
  const defaults = state?.desktopId === desktopId ? state.defaults : EMPTY_DEFAULTS;
  const update = useCallback(
    (patch: (current: DefaultModels) => DefaultModels) => saveDefaultModels(desktopId, patch(defaults)),
    [desktopId, defaults],
  );
  return { defaults, loaded: state?.desktopId === desktopId, update };
}

/** `defaults` with this model remembered for its provider and chosen as the starting provider. */
export function withDefaultModel(defaults: DefaultModels, saved: SavedModel): DefaultModels {
  return { ...defaults, harness: saved.harness, models: { ...defaults.models, [saved.harness]: saved } };
}

/**
 * What a new chat starts with: the saved provider's model when the catalog still lists it (its settings
 * carried over where they still apply), else what the desktop reports as current. The saved access
 * replaces the desktop's when it is set.
 */
export function startingSelection(catalog: ModelCatalog, defaults: DefaultModels): ModelSelectionInput {
  const base = currentSelection(catalog);
  const saved = defaults.harness ? defaults.models[defaults.harness] : undefined;
  const entry = saved ? catalog.models.find((model) => model.harness === saved.harness && model.id === saved.model) : undefined;
  const chosen: ModelSelectionInput =
    saved && entry
      ? { harness: entry.harness, model: entry.id, modelSettings: mergeModelSettings(entry, saved.modelSettings), runtimeMode: base.runtimeMode }
      : base;
  return defaults.runtimeMode ? { ...chosen, runtimeMode: defaults.runtimeMode } : chosen;
}
