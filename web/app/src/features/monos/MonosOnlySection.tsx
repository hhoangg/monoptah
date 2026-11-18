// F-13: the "Monos" group of the settings page: the device-local "Monos only" preference. When it is on, the
// workspace home shows the Mono list (`MonoHome`) instead of projects (WS-T renders it, see the report).
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Switch } from "@/ui/switch";
import { useMonoPrefs } from "./monoPrefs";

export function MonosOnlySection() {
  const monosOnly = useMonoPrefs((state) => state.monosOnly);
  const setMonosOnly = useMonoPrefs((state) => state.setMonosOnly);
  return (
    <SettingsCard title="Monos" footnote="Show only your Monos on the home screen, as a messaging app lists its chats. Kept on this device.">
      <SettingsRow
        icon="chat"
        title="Monos only"
        trailing={<Switch checked={monosOnly} onCheckedChange={setMonosOnly} aria-label="Monos only" />}
      />
    </SettingsCard>
  );
}
