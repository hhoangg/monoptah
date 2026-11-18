// U-07 (F-56): the default model and access of one desktop, rendered in the per-desktop settings
// through the `desktop-settings-sections` slot (registered in ./register.ts). New chats of that desktop
// start with these (see `startingSelection` in ./defaultModel.ts). Access offers the sendable modes only.
import { useState } from "react";
import type { SendableRuntimeMode } from "@monoptah/protocol";
import { LinkProvider } from "@/link";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { accessMode } from "./access";
import { AccessPicker } from "./AccessPicker";
import { useDefaultModels, startingSelection, withDefaultModel, EMPTY_DEFAULTS } from "./defaultModel";
import { useAccountUsage, useModelCatalog } from "./hooks";
import { ModelPicker } from "./ModelPicker";
import type { ModelSelection } from "./modelSettings";

export function DefaultModelSection({ desktopId }: { desktopId: string }) {
  return (
    <LinkProvider desktopId={desktopId}>
      <DefaultModelRows desktopId={desktopId} />
    </LinkProvider>
  );
}

function DefaultModelRows({ desktopId }: { desktopId: string }) {
  const catalog = useModelCatalog();
  const { defaults, loaded, update } = useDefaultModels(desktopId);
  const [open, setOpen] = useState(false);
  const data = catalog.data;
  const start = data ? startingSelection(data, defaults) : null;
  const usage = useAccountUsage(open && start ? start.harness : undefined);
  const isSet = defaults.harness !== null || defaults.runtimeMode !== null;

  if (!data || !start) {
    return (
      <SettingsCard title="Models">
        <SettingsRow
          icon="sparkle"
          title="Default model"
          description={catalog.isError ? "Needs a connection to the desktop" : "Loading models..."}
        />
      </SettingsCard>
    );
  }

  const model = data.models.find((entry) => entry.id === start.model);
  const savedHarness = defaults.harness;
  const pick = (next: ModelSelection) => {
    const entry = data.models.find((candidate) => candidate.id === next.model);
    void update((current) => withDefaultModel(current, { harness: next.harness, model: next.model, modelSettings: next.modelSettings, name: entry?.name ?? next.model }));
  };
  const setAccess = (mode: SendableRuntimeMode) => void update((current) => ({ ...current, runtimeMode: mode }));

  return (
    <SettingsCard title="Models" footnote="New chats on this desktop start with these. Full access can only be set on the desktop.">
      <SettingsRow
        icon="sparkle"
        title="Default model"
        description={savedHarness ? "Set for this desktop" : "The desktop decides"}
        trailing={
          <span className="inline-flex items-center gap-1.5">
            <HarnessIcon harness={start.harness} className="size-3.5" />
            {model?.name ?? start.model}
          </span>
        }
        onClick={() => setOpen(true)}
        disabled={!loaded}
      />
      <SettingsRow
        icon="shield"
        title="Default access"
        description={defaults.runtimeMode ? accessMode(defaults.runtimeMode).hint : "The desktop decides"}
        trailing={<AccessPicker value={defaults.runtimeMode ?? start.runtimeMode} onChange={setAccess} variant="plain" side="bottom" />}
      />
      {isSet ? <SettingsRow icon="refresh" title="Use the desktop defaults" onClick={() => void update(() => EMPTY_DEFAULTS)} /> : null}
      <ModelPicker
        open={open}
        onOpenChange={setOpen}
        catalog={data}
        value={start}
        onChange={pick}
        defaultModelId={savedHarness ? defaults.models[savedHarness]?.model : undefined}
        usage={usage}
        showAccess={false}
      />
    </SettingsCard>
  );
}
