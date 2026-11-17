import { useAppearance } from "@/lib/appearance";
import { SegmentedControl } from "@/ui/SegmentedControl";
import { SettingsCard } from "@/ui/SettingsCard";
import type { Appearance } from "@/styles/themes";

const OPTIONS = [
  { value: "system", label: "System", icon: "appearance" },
  { value: "light", label: "Light", icon: "sun" },
  { value: "dark", label: "Dark", icon: "moon" },
] as const satisfies ReadonlyArray<{ value: Appearance; label: string; icon: "appearance" | "sun" | "moon" }>;

/** Colour scheme (F-87): System, Light or Dark. Stored on this device only (`monoptah.appearance`). */
export function AppearanceSection() {
  const appearance = useAppearance((state) => state.appearance);
  const setAppearance = useAppearance((state) => state.setAppearance);
  return (
    <SettingsCard title="Appearance" footnote="System follows your device setting. This choice is saved on this device only.">
      <div className="px-1.5 py-2">
        <SegmentedControl label="Colour scheme" options={OPTIONS} value={appearance} onChange={setAppearance} />
      </div>
    </SettingsCard>
  );
}
