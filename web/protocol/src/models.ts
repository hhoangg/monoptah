// Copied from src/features/sessions/model/models.ts at ca73ac2b
// Types and effort helpers only.
// The wire types live in wire.ts and are re-exported here; this file adds the pure helpers the
// pickers use. There is no static model table (the catalog comes from `models.list`) and no
// storage access: the web keeps these preferences under the `monoptah.` keys listed below.
import type { z } from "zod";
import type { CatalogModel, ModelSettingSchema } from "./methods.ts";

export {
  CatalogModelSchema,
  ModelCatalogSchema,
  ModelChoiceSchema,
  ModelSettingSchema,
  RuntimeModeSchema,
  SendableRuntimeModeSchema,
} from "./methods.ts";
export type { CatalogModel, ModelCatalog, ModelChoice, RuntimeMode, SendableRuntimeMode } from "./methods.ts";

/** One select or toggle setting of a model (effort, fast mode, thinking, ...). */
export type ModelSetting = z.infer<typeof ModelSettingSchema>;
export type ModelSettingChoice = ModelSetting["options"][number];
/** A model of the live `models.list` catalog (desktop `AgentModel`). */
export type AgentModel = CatalogModel;

/** Browser storage keys of the model preferences (the upstream `favoriteModels` and friends, renamed). */
export const MODEL_STORAGE_KEYS = {
  favorites: "monoptah.favoriteModels",
  lastModel: "monoptah.lastModel",
  lastModelSettings: "monoptah.lastModelSettings",
  recentModels: "monoptah.recentModels",
  modelPickerTab: "monoptah.modelPickerTab",
} as const;

/** Cursor CLI uses `extra-high`; Claude uses `xhigh`. */
const SETTING_VALUE_ALIASES: Record<string, string> = {
  "extra-high": "xhigh",
  xhigh: "extra-high",
};

const EFFORT_SETTING_IDS = new Set([
  "effort",
  "reasoning",
  "reasoningEffort",
  // Pi and OMP expose their reasoning level as a `thinking` select.
  "thinking",
  // OpenCode exposes reasoning levels as `variant`; treat it as effort so the
  // standalone effort control and dedup behave like Codex/Cursor/Grok.
  "variant",
]);

/** True for the select setting ids that control reasoning effort. */
export function isEffortSettingId(id: string): boolean {
  return EFFORT_SETTING_IDS.has(id);
}

/** The select setting that controls reasoning effort for this model, if any. */
export function modelEffortSetting(model: Pick<AgentModel, "settings">): ModelSetting | undefined {
  return model.settings.find((setting) => setting.kind === "select" && EFFORT_SETTING_IDS.has(setting.id));
}

export function modelEffortLabel(
  model: Pick<AgentModel, "settings">,
  values?: Record<string, string>,
): string | undefined {
  const setting = modelEffortSetting(model);
  if (!setting) return undefined;
  const value = values?.[setting.id] ?? setting.value;
  return setting.options.find((option) => option.value === value)?.label ?? value;
}

export function defaultModelSettings(model: Pick<AgentModel, "settings">): Record<string, string> {
  const settings: Record<string, string> = {};
  for (const setting of model.settings) settings[setting.id] = setting.value;
  return settings;
}

function compatibleSettingValue(setting: ModelSetting, value: string | undefined): string | undefined {
  if (value == null) return undefined;
  if (setting.options.some((option) => option.value === value)) return value;
  const alias = SETTING_VALUE_ALIASES[value];
  if (alias && setting.options.some((option) => option.value === alias)) return alias;
  return undefined;
}

/** Settings for `model`: each current value it still offers, its default for the rest. */
export function mergeModelSettings(
  model: Pick<AgentModel, "settings">,
  current?: Record<string, string>,
): Record<string, string> {
  const next = defaultModelSettings(model);
  if (!current) return next;
  for (const setting of model.settings) {
    const value = compatibleSettingValue(setting, current[setting.id]);
    if (value != null) next[setting.id] = value;
  }
  return next;
}

/** Last chosen effort or fast mode, applied to any model that supports those values. */
export function preferredModelSettings(
  model: Pick<AgentModel, "settings">,
  current?: Record<string, string>,
  remembered?: Record<string, string>,
): Record<string, string> {
  return mergeModelSettings(model, { ...current, ...remembered });
}

/**
 * The remembered settings after a choice. `overwrite` lets the incoming values win, `fill` keeps
 * what is already remembered and only adds what is new.
 */
export function rememberModelSettings(
  previous: Record<string, string>,
  incoming: Record<string, string>,
  mode: "overwrite" | "fill" = "overwrite",
): Record<string, string> {
  const prev = parseStringRecord(previous);
  const next = parseStringRecord(incoming);
  return mode === "fill" ? { ...next, ...prev } : { ...prev, ...next };
}

/** Only string values survive, so a damaged stored record cannot reach the wire. */
export function parseStringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [key, item] of Object.entries(value)) {
    if (typeof item === "string") out[key] = item;
  }
  return out;
}

// --- Ported from mobile/src/features/models/modelChoice.ts ---

/** Fast mode, as a toggle or as Codex's service tier, with its on and off values. */
export function fastSetting(
  model: Pick<AgentModel, "settings"> | undefined,
): { setting: ModelSetting; on: string; off: string } | undefined {
  const setting = model?.settings.find(
    (item) => item.id === "fast" || item.id === "serviceTier" || /^fast( mode)?$/i.test(item.label),
  );
  if (!setting) return undefined;
  const on = setting.options.find((option) => /^(true|fast|priority)$/i.test(option.value));
  const off = setting.options.find((option) => /^(false|default|standard|normal)$/i.test(option.value));
  return on && off ? { setting, on: on.value, off: off.value } : undefined;
}

/** The model's other settings, shown as plain choices under effort and fast mode. */
export function otherSettings(model: Pick<AgentModel, "settings"> | undefined): ModelSetting[] {
  const effort = model ? modelEffortSetting(model) : undefined;
  const fast = fastSetting(model)?.setting;
  return (model?.settings ?? []).filter((setting) => setting !== effort && setting !== fast && setting.options.length > 1);
}

/** The value a setting has in the choice, or its default. */
export function settingValue(modelSettings: Record<string, string>, setting: ModelSetting): string {
  return modelSettings[setting.id] ?? setting.value;
}

/** The choice with another model, its settings carried over where they still apply. */
export function withModel<T extends { harness: string; model: string; modelSettings: Record<string, string> }>(
  choice: T,
  model: AgentModel,
): T {
  return {
    ...choice,
    harness: model.harness,
    model: model.id,
    modelSettings: mergeModelSettings(model, choice.modelSettings),
  };
}
