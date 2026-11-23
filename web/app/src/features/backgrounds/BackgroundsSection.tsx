import { SETTINGS_ANCHOR, SETTINGS_DIVIDER_BASE, SettingsCard } from "@/ui/SettingsCard";
import { BackgroundControls } from "./BackgroundControls";
import { useAllBackgrounds } from "./useBackground";

/**
 * The "Backgrounds" settings group (F-90): the empty-session photos set on this device, with opacity and
 * remove. A photo is added from a project (see `BackgroundControls`, mounted by `features/layout/BackgroundDialog.tsx`).
 * Each row's divider starts where the project name does: 4px padding + 48px thumbnail + 12px gap (see BackgroundControls).
 */
export function BackgroundsSection() {
  const records = useAllBackgrounds();
  return (
    <SettingsCard
      id="backgrounds"
      className={SETTINGS_ANCHOR}
      title="Backgrounds"
      footnote="Photos shown behind an empty session, dithered on this device and saved here only. Add one from a project: use Background in the project bar or the search."
    >
      {records.length === 0 ? (
        <p className="px-1 py-3 text-row text-muted-text">No backgrounds yet.</p>
      ) : (
        <ul className="flex flex-col">
          {records.map((record) => (
            <li key={record.projectId} className={`relative ${SETTINGS_DIVIDER_BASE} before:left-[64px]`}>
              <BackgroundControls projectId={record.projectId} projectName={record.projectName} />
            </li>
          ))}
        </ul>
      )}
    </SettingsCard>
  );
}
