// App-side mirror of the pure helpers of web/protocol/src/models.ts (that file is not exported by
// `@monoptah/protocol` yet). `modelSettings.test.ts` checks every function against the protocol file
// so the two cannot drift. `full-access` never appears as a choice here: access lives in ./access.ts.
import type { CatalogModel, ModelCatalog, RuntimeMode, SendableRuntimeMode } from "@monoptah/protocol";
import { sendableAccess } from "./access";

export type ModelSetting = CatalogModel["settings"][number];

/** What a session or a new chat runs with, as the desktop reports it (access may be `full-access`). */
export interface ModelSelectionInput {
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  runtimeMode: RuntimeMode;
  providerAccountId?: string | undefined;
}

/**
 * What the pickers emit. `runtimeMode` is typed sendable: a session that runs in `full-access`
 * emits no access at all, so the desktop keeps it as it is and this browser never sends it.
 */
export interface ModelSelection {
  harness: string;
  model: string;
  modelSettings: Record<string, string>;
  runtimeMode?: SendableRuntimeMode | undefined;
  providerAccountId?: string | undefined;
}

/** Efforts past "high" are tinted as the desktop's effort tiles warm up. */
export const HOT_EFFORT = /^(xhigh|max|ultra|ultracode|ultrathink)$/;

const EFFORT_SETTING_IDS = new Set(["effort", "reasoning", "reasoningEffort", "thinking", "variant"]);
const SETTING_VALUE_ALIASES: Record<string, string> = { "extra-high": "xhigh", xhigh: "extra-high" };

export function isEffortSettingId(id: string): boolean {
  return EFFORT_SETTING_IDS.has(id);
}

export function modelEffortSetting(model: Pick<CatalogModel, "settings"> | undefined): ModelSetting | undefined {
  return model?.settings.find((setting) => setting.kind === "select" && EFFORT_SETTING_IDS.has(setting.id));
}

export function defaultModelSettings(model: Pick<CatalogModel, "settings">): Record<string, string> {
  const settings: Record<string, string> = {};
  for (const setting of model.settings) settings[setting.id] = setting.value;
  return settings;
}

function compatibleValue(setting: ModelSetting, value: string | undefined): string | undefined {
  if (value == null) return undefined;
  if (setting.options.some((option) => option.value === value)) return value;
  const alias = SETTING_VALUE_ALIASES[value];
  if (alias && setting.options.some((option) => option.value === alias)) return alias;
  return undefined;
}

/** Settings for `model`: each current value it still offers, its default for the rest. */
export function mergeModelSettings(model: Pick<CatalogModel, "settings">, current?: Record<string, string>): Record<string, string> {
  const next = defaultModelSettings(model);
  if (!current) return next;
  for (const setting of model.settings) {
    const value = compatibleValue(setting, current[setting.id]);
    if (value != null) next[setting.id] = value;
  }
  return next;
}

/** Only string values survive, so a damaged stored record cannot reach the wire. */
export function parseStringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [key, item] of Object.entries(value)) if (typeof item === "string") out[key] = item;
  return out;
}

export function fastSetting(model: Pick<CatalogModel, "settings"> | undefined): { setting: ModelSetting; on: string; off: string } | undefined {
  const setting = model?.settings.find((item) => item.id === "fast" || item.id === "serviceTier" || /^fast( mode)?$/i.test(item.label));
  if (!setting) return undefined;
  const on = setting.options.find((option) => /^(true|fast|priority)$/i.test(option.value));
  const off = setting.options.find((option) => /^(false|default|standard|normal)$/i.test(option.value));
  return on && off ? { setting, on: on.value, off: off.value } : undefined;
}

/** The model's other settings, shown as plain choices under effort and fast mode. */
export function otherSettings(model: Pick<CatalogModel, "settings"> | undefined): ModelSetting[] {
  const effort = modelEffortSetting(model);
  const fast = fastSetting(model)?.setting;
  return (model?.settings ?? []).filter((setting) => setting !== effort && setting !== fast && setting.options.length > 1);
}

/** The value a setting has in the choice, or its default. */
export function settingValue(modelSettings: Record<string, string>, setting: ModelSetting): string {
  return modelSettings[setting.id] ?? setting.value;
}

/** The label of the effort the choice has, for the toolbar pill. */
export function effortLabel(model: Pick<CatalogModel, "settings"> | undefined, modelSettings: Record<string, string>): string | undefined {
  const setting = modelEffortSetting(model);
  if (!setting) return undefined;
  const value = settingValue(modelSettings, setting);
  return setting.options.find((option) => option.value === value)?.label ?? value;
}

/** The selection with another model, its settings carried over where they still apply. */
export function withModel<T extends { harness: string; model: string; modelSettings: Record<string, string> }>(choice: T, model: CatalogModel): T {
  return { ...choice, harness: model.harness, model: model.id, modelSettings: mergeModelSettings(model, choice.modelSettings) };
}

/** The selection the desktop reports as current, as the pickers show it. */
export function currentSelection(catalog: ModelCatalog): ModelSelectionInput {
  const { harness, model, modelSettings, runtimeMode, providerAccountId } = catalog.current;
  return { harness, model, modelSettings, runtimeMode, providerAccountId };
}

/**
 * The emitted form of a selection. A reported `full-access` (or any unknown mode) is dropped, never
 * carried forward; `runtimeMode` is only set when the user picked one of the sendable modes.
 */
export function toEmitted(input: ModelSelectionInput, runtimeMode?: SendableRuntimeMode): ModelSelection {
  const sendable = runtimeMode ?? sendableAccess(input.runtimeMode) ?? undefined;
  return {
    harness: input.harness,
    model: input.model,
    modelSettings: input.modelSettings,
    ...(sendable ? { runtimeMode: sendable } : {}),
    ...(input.providerAccountId ? { providerAccountId: input.providerAccountId } : {}),
  };
}
