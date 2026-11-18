import { SettingsCard } from "@/ui/SettingsCard";
import { BackgroundControls } from "./BackgroundControls";
import { useAllBackgrounds } from "./useBackground";

/**
 * The "Backgrounds" settings group (F-90): the empty-session photos set on this device, with opacity and
 * remove. A photo is added from a project (see `BackgroundControls`, mounted by the project screen).
 */
export function BackgroundsSection() {
  const records = useAllBackgrounds();
  return (
    <SettingsCard
      title="Backgrounds"
      footnote="Photos shown behind an empty session, dithered on this device and saved here only. Add one from a project's new session screen."
    >
      {records.length === 0 ? (
        <p className="px-1 py-3 text-row text-muted-text">No backgrounds yet.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-stroke">
          {records.map((record) => (
            <li key={record.projectId}>
              <BackgroundControls projectId={record.projectId} projectName={record.projectName} />
            </li>
          ))}
        </ul>
      )}
    </SettingsCard>
  );
}
