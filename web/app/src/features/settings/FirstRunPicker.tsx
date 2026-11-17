import { useState } from "react";
import { useLinkDesktopId } from "@/link/LinkProvider";
import { pickerSeenKey, useDesktopLink } from "./desktopLink";
import { ProjectsPicker } from "./ProjectsPicker";

export { pickerSeenKey };

export function hasSeenPicker(desktopId: string): boolean {
  try {
    return window.localStorage.getItem(pickerSeenKey(desktopId)) === "1";
  } catch {
    return true;
  }
}

function markPickerSeen(desktopId: string): void {
  try {
    window.localStorage.setItem(pickerSeenKey(desktopId), "1");
  } catch {
    // Storage unavailable: the picker may show again next time, which is harmless.
  }
}

/**
 * The `first-run-picker` slot of the `/d/:desktopId` layout: the F-10 picker as an overlay, once per
 * desktop on this browser. It waits for a connected link (a pending or offline desktop has nothing to
 * pick yet). Closing it any way counts as "Choose later": the selection the desktop granted stays.
 */
export function FirstRunPicker() {
  const desktopId = useLinkDesktopId();
  // Keyed by desktop: the layout is not remounted when the param changes, so each desktop gets its own "seen" state.
  return <FirstRunPickerFor key={desktopId} desktopId={desktopId} />;
}

function FirstRunPickerFor({ desktopId }: { desktopId: string }) {
  const { link, status } = useDesktopLink(desktopId);
  const [seen, setSeen] = useState(() => hasSeenPicker(desktopId));
  if (!link || seen || status !== "connected") return null;
  return (
    <ProjectsPicker
      desktopId={desktopId}
      link={link}
      mode="first-run"
      open
      onOpenChange={(open) => {
        if (open) return;
        markPickerSeen(desktopId);
        setSeen(true);
      }}
    />
  );
}
