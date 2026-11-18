// Ported from mobile/src/features/models/ModelPicker.tsx (F-51 to F-54): every provider's models in
// folding sections with a search, the chosen model's options screen, the account picker. A vaul sheet
// under 640px and a dialog from 640px (ResponsiveOverlay). The component holds no data: the catalog,
// the current selection and `onChange` are props, so the composer toolbar, the new-session page and the
// default-model settings all use it. `onChange` only ever receives a `ModelSelection`, whose access is
// typed sendable (see ./modelSettings.ts).
import { useState } from "react";
import type { ModelCatalog } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Check, ChevronDown, ChevronLeft, ChevronRight, Search } from "@/ui/icons";
import { Input } from "@/ui/input";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { AccountPicker } from "./AccountPicker";
import type { AccountUsage } from "./hooks";
import { ModelOptions } from "./ModelOptions";
import { effortLabel, fastSetting, otherSettings, modelEffortSetting, settingValue, toEmitted, withModel, type ModelSelection, type ModelSelectionInput } from "./modelSettings";
import { isFullAccess, accessMode, FULL_ACCESS_DISPLAY } from "./access";

export interface ModelPickerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  catalog: ModelCatalog;
  value: ModelSelectionInput;
  onChange: (next: ModelSelection) => void;
  /** A running session (not a new chat): switching accounts opens a new chat instead. */
  existingSession?: boolean;
  /** Called instead of `onChange` when the account of a running session is picked. */
  onSwitchAccount?: (accountId: string, selection: ModelSelection) => void;
  /** The model id marked "Default" (U-07). */
  defaultModelId?: string | undefined;
  /** Usage meters of the chosen provider's accounts (`useAccountUsage`). */
  usage?: Pick<AccountUsage, "accounts" | "refreshing" | "error" | "refresh">;
  /** A read-only device or a missing link: browsing works, nothing changes. */
  disabled?: boolean;
  busy?: boolean;
  /** Hide the access row (default-model editor sets access elsewhere). */
  showAccess?: boolean;
}

const NO_USAGE = { accounts: {}, refreshing: false, error: null, refresh: () => undefined };

/** The button text of the toolbar pill: model name and its effort. */
export function summarizeSelection(catalog: ModelCatalog, value: ModelSelectionInput): { name: string; effort: string | undefined } {
  const model = catalog.models.find((entry) => entry.id === value.model);
  return { name: model?.name ?? value.model, effort: effortLabel(model, value.modelSettings) };
}

export function ModelPicker(props: ModelPickerProps) {
  const { open, onOpenChange, catalog, value } = props;
  const [view, setView] = useState<"list" | "options">("list");
  const model = catalog.models.find((entry) => entry.id === value.model);
  return (
    <ResponsiveOverlay
      open={open}
      onOpenChange={(next) => {
        if (!next) setView("list");
        onOpenChange(next);
      }}
      title={view === "list" ? "Model" : `Options for ${model?.name ?? value.model}`}
    >
      {view === "list" ? <ModelList {...props} onOpenOptions={() => setView("options")} /> : <OptionsView {...props} onBack={() => setView("list")} />}
    </ResponsiveOverlay>
  );
}

function OptionsView({ catalog, value, onChange, disabled, busy, showAccess, onBack }: ModelPickerProps & { onBack: () => void }) {
  const model = catalog.models.find((entry) => entry.id === value.model);
  return (
    <div className="flex flex-col gap-3">
      <button type="button" onClick={onBack} className="inline-flex min-h-9 items-center gap-1 self-start rounded-row pr-2 text-label text-muted-text hover:text-content">
        <ChevronLeft size={16} />
        Models
      </button>
      <ModelOptions model={model} value={value} onChange={onChange} showAccess={showAccess ?? true} disabled={disabled ?? false} busy={busy ?? false} />
    </div>
  );
}

function optionsSummary(catalog: ModelCatalog, value: ModelSelectionInput, showAccess: boolean): string {
  const model = catalog.models.find((entry) => entry.id === value.model);
  const parts: string[] = [];
  const effort = effortLabel(model, value.modelSettings);
  if (effort) parts.push(effort);
  const fast = fastSetting(model);
  if (fast && settingValue(value.modelSettings, fast.setting) === fast.on) parts.push("Fast");
  for (const setting of otherSettings(model)) {
    const current = settingValue(value.modelSettings, setting);
    const label = setting.options.find((option) => option.value === current)?.label;
    if (label) parts.push(label);
  }
  if (showAccess && value.harness !== "fx") parts.push(isFullAccess(value.runtimeMode) ? FULL_ACCESS_DISPLAY.label : accessMode(value.runtimeMode).label);
  return parts.join(" · ");
}

function ModelList({
  catalog,
  value,
  onChange,
  existingSession = false,
  onSwitchAccount,
  defaultModelId,
  usage = NO_USAGE,
  disabled = false,
  showAccess = true,
  onOpenOptions,
}: ModelPickerProps & { onOpenOptions: () => void }) {
  const [query, setQuery] = useState("");
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  const needle = query.trim().toLowerCase();
  const searching = needle.length > 0;
  const model = catalog.models.find((entry) => entry.id === value.model);
  const hasOptions = Boolean(modelEffortSetting(model) || fastSetting(model) || otherSettings(model).length > 0 || (showAccess && value.harness !== "fx"));

  const accounts = catalog.accounts?.filter((account) => account.provider === value.harness) ?? [];
  const accountId = value.providerAccountId ?? catalog.selectedAccounts?.[value.harness] ?? "default";

  const sections = catalog.harnesses
    .map((harness) => ({
      ...harness,
      models: catalog.models.filter(
        (entry) => entry.harness === harness.id && (!searching || [entry.name, entry.id, entry.provider ?? ""].some((field) => field.toLowerCase().includes(needle))),
      ),
    }))
    .filter((section) => !searching || section.models.length > 0);
  const isOpen = (harness: string) => searching || !(folded[harness] ?? harness !== value.harness);

  const pick = (entry: ModelCatalog["models"][number]) => {
    if (disabled || entry.id === value.model) return;
    // An account belongs to one provider: another provider starts with its own selection.
    const base = entry.harness === value.harness ? value : { ...value, providerAccountId: undefined };
    onChange(toEmitted(withModel(base, entry)));
  };
  const pickAccount = (id: string) => {
    if (disabled) return;
    const next = toEmitted({ ...value, providerAccountId: id });
    if (existingSession && onSwitchAccount) onSwitchAccount(id, next);
    else onChange(next);
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="relative">
        <Search size={16} className="pointer-events-none absolute top-1/2 left-3.5 -translate-y-1/2 text-muted-text" />
        <Input type="search" aria-label="Search models" placeholder="Search models" value={query} onChange={(event) => setQuery(event.target.value)} className="pl-10" />
      </div>

      {!searching && accounts.length > 0 ? (
        <section className="flex flex-col gap-2">
          <h3 className="px-3 text-label font-medium text-muted-text">Account</h3>
          <AccountPicker
            key={value.harness}
            accounts={accounts}
            value={accountId}
            existingSession={existingSession}
            onPick={pickAccount}
            usage={usage}
            canRefresh={!disabled}
          />
        </section>
      ) : null}

      {!searching && hasOptions ? (
        <section className="flex flex-col gap-2">
          <h3 className="px-3 text-label font-medium text-muted-text">Selected provider</h3>
          <button
            type="button"
            aria-label={`Options for ${model?.name ?? value.model}: ${optionsSummary(catalog, value, showAccess)}`}
            onClick={onOpenOptions}
            className="flex min-h-16 items-center gap-3 rounded-card bg-fill px-3 py-3 text-left hover:bg-selection"
          >
            <HarnessIcon harness={value.harness} className="size-5" />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-row font-medium text-content">{model?.name ?? value.model}</span>
              <span className={cn("block truncate text-meta", isFullAccess(value.runtimeMode) ? "text-caution" : "text-muted-text")}>
                {optionsSummary(catalog, value, showAccess) || "Options"}
              </span>
            </span>
            <span className="text-label text-muted-text">Options</span>
            <ChevronRight size={14} className="text-muted-text" />
          </button>
        </section>
      ) : null}

      {sections.length === 0 ? <p className="px-1 text-label text-muted-text">No matching models.</p> : null}
      <div className="flex flex-col gap-1">
        {sections.map((section) => {
          const sectionOpen = isOpen(section.id);
          return (
            <div key={section.id} className={cn("rounded-card", sectionOpen && "bg-fill")}>
              <button
                type="button"
                aria-expanded={sectionOpen}
                disabled={searching}
                onClick={() => setFolded((current) => ({ ...current, [section.id]: sectionOpen }))}
                className="flex min-h-12 w-full items-center gap-3 px-3 text-left"
              >
                <HarnessIcon harness={section.id} className="size-[18px]" />
                <span className="text-row font-medium text-content">{section.label}</span>
                {sectionOpen ? null : <span className="text-label text-muted-text">{section.models.length}</span>}
                <span className="flex-1" />
                {searching ? null : <ChevronDown size={16} className={cn("text-muted-text transition-transform motion-reduce:transition-none", sectionOpen && "rotate-180")} />}
              </button>
              {sectionOpen ? (
                <ul aria-label={`${section.label} models`}>
                  {section.models.length === 0 ? <li className="border-t border-stroke px-3 py-3 text-label text-muted-text">No models from this provider yet.</li> : null}
                  {section.models.map((entry) => {
                    const selected = entry.id === value.model;
                    return (
                      <li key={entry.id} className="border-t border-stroke">
                        <button
                          type="button"
                          aria-pressed={selected}
                          disabled={disabled}
                          onClick={() => pick(entry)}
                          className="flex min-h-12 w-full items-center gap-3 px-3 py-2 text-left hover:bg-selection disabled:opacity-60"
                        >
                          <span className="min-w-0 flex-1">
                            <span className="flex items-center gap-2">
                              <span className={cn("truncate text-row text-content", selected && "font-semibold")}>{entry.name}</span>
                              {entry.id === defaultModelId ? <span className="rounded-pill bg-selection px-2 text-meta text-muted-text">Default</span> : null}
                            </span>
                            {entry.provider ? <span className="block truncate text-meta text-muted-text">{entry.provider}</span> : null}
                          </span>
                          {selected ? <Check size={18} className="shrink-0 text-brand-text" /> : null}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
