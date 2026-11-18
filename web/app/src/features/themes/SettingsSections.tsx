import { BackgroundsSection } from "@/features/backgrounds";
import { ThemeSection } from "./ThemeSection";

/**
 * The WS-P part of the `settings-sections` slot. Later tasks add their groups here.
 *
 * `MonosOnlySection` (the F-13 "Monos only" switch) is not shown yet: nothing reads the preference until WS-T's
 * workspace home renders `MonoHome` when `useMonoPrefs().monosOnly` is on, and a switch that changes nothing is not
 * honest. Add `<MonosOnlySection />` here when that lands (see the batch C fix report, "Needed outside WS-P").
 */
export function WsPSettingsSections() {
  return (
    <>
      <ThemeSection />
      <BackgroundsSection />
    </>
  );
}
