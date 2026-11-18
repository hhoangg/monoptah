// The options of the chosen model (F-52, F-53): effort slider, fast mode switch, the model's other
// settings as radio choices, and access. Ported from ModelOptions in mobile/src/features/models/ModelPicker.tsx.
// Every change is emitted through `toEmitted`, so the emitted selection never carries `full-access`.
import type { CatalogModel, SendableRuntimeMode } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { Switch } from "@/ui/switch";
import { Check, Shield } from "@/ui/icons";
import { FULL_ACCESS_DISPLAY, isFullAccess } from "./access";
import { AccessPicker } from "./AccessPicker";
import { EffortSlider } from "./EffortSlider";
import {
  fastSetting,
  modelEffortSetting,
  otherSettings,
  settingValue,
  toEmitted,
  type ModelSelection,
  type ModelSelectionInput,
} from "./modelSettings";

export interface ModelOptionsProps {
  model: CatalogModel | undefined;
  value: ModelSelectionInput;
  onChange: (next: ModelSelection) => void;
  /** Hide access (fx has no permissions, a default-model editor shows it elsewhere). */
  showAccess?: boolean;
  /** A read-only device or a missing link: nothing can change. */
  disabled?: boolean;
  /** A turn is running: access changes apply to the next turn. */
  busy?: boolean;
}

export function ModelOptions({ model, value, onChange, showAccess = true, disabled = false, busy = false }: ModelOptionsProps) {
  const effort = modelEffortSetting(model);
  const fast = fastSetting(model);
  const others = otherSettings(model);
  const fullAccess = isFullAccess(value.runtimeMode);

  const setSetting = (id: string, next: string) =>
    onChange(toEmitted({ ...value, modelSettings: { ...value.modelSettings, [id]: next } }));
  const setAccess = (mode: SendableRuntimeMode) => onChange(toEmitted(value, mode));

  const showAccessRow = showAccess && value.harness !== "fx";
  if (!effort && !fast && others.length === 0 && !showAccessRow) {
    return <p className="px-1 text-label text-muted-text">This model has no options.</p>;
  }

  return (
    <div className="flex flex-col gap-3">
      {effort ? (
        <section className="rounded-card bg-fill p-4">
          <EffortSlider
            label={effort.label}
            options={effort.options}
            value={settingValue(value.modelSettings, effort)}
            onChange={(next) => setSetting(effort.id, next)}
            disabled={disabled}
          />
          {effort.description ? <p className="mt-2 text-meta text-muted-text">{effort.description}</p> : null}
        </section>
      ) : null}

      {fast ? (
        <section className="flex items-center gap-3 rounded-card bg-fill p-4">
          <span className="min-w-0 flex-1">
            <span id="fast-mode-label" className="block text-row font-medium text-content">
              Fast mode
            </span>
            <span className="block text-meta text-muted-text">Quicker replies, at a higher cost.</span>
          </span>
          <Switch
            aria-labelledby="fast-mode-label"
            checked={settingValue(value.modelSettings, fast.setting) === fast.on}
            disabled={disabled}
            onCheckedChange={(on) => setSetting(fast.setting.id, on ? fast.on : fast.off)}
          />
        </section>
      ) : null}

      {others.map((setting) => {
        const current = settingValue(value.modelSettings, setting);
        return (
          <section key={setting.id} className="rounded-card bg-fill p-4" role="radiogroup" aria-label={setting.label}>
            <p className="text-row font-medium text-content">{setting.label}</p>
            {setting.description ? <p className="text-meta text-muted-text">{setting.description}</p> : null}
            <div className="mt-2 flex flex-col">
              {setting.options.map((option) => {
                const checked = option.value === current;
                return (
                  <button
                    key={option.value}
                    type="button"
                    role="radio"
                    aria-checked={checked}
                    disabled={disabled}
                    onClick={() => !checked && setSetting(setting.id, option.value)}
                    className={cn(
                      "flex min-h-11 items-center gap-2 rounded-row px-2 text-left text-row text-content hover:bg-selection disabled:opacity-40 pointer-fine:min-h-9",
                      checked && "font-medium",
                    )}
                  >
                    <span className="min-w-0 flex-1">{option.label}</span>
                    {checked ? <Check size={16} /> : null}
                  </button>
                );
              })}
            </div>
          </section>
        );
      })}

      {showAccessRow ? (
        <section className="rounded-card bg-fill p-4" aria-label="Access">
          <div className="flex items-center justify-between gap-3">
            <span className="text-row font-medium text-content">Access</span>
            <AccessPicker value={value.runtimeMode} onChange={setAccess} disabled={disabled} busy={busy} side="bottom" />
          </div>
          {fullAccess ? (
            <p className="mt-2 flex items-center gap-2 text-meta text-muted-text" data-full-access-note>
              <Shield size={14} className="shrink-0" />
              {FULL_ACCESS_DISPLAY.note}
            </p>
          ) : null}
        </section>
      ) : null}
    </div>
  );
}
