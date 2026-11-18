// The region under the transcript where Task 5 mounts the composer and the question card.
//
// Mount point: pass the composer as `children`. The region renders, top to bottom, the `composer-top` slot
// host (the approval card and the cloud banners of WS-C, W-08), `above` (for example the question card),
// then `children`. With nothing passed it is an empty strip, so the transcript fills the screen.
import type { ReactNode } from "react";
import { useKeyboardInset } from "@/features/composer/useKeyboardInset";
import { ComposerTopSlot } from "./slots";

export interface ComposerRegionProps {
  desktopId: string;
  sessionId: string;
  /** Between the slot and the composer: the question card (F-58). */
  above?: ReactNode;
  /** The composer. */
  children?: ReactNode;
}

export function ComposerRegion({ desktopId, sessionId, above, children }: ComposerRegionProps) {
  // The safe-area inset is the home-indicator gap; while the keyboard is up the composer sits on the keyboard, not on that gap.
  const keyboardInset = useKeyboardInset();
  return (
    <div data-testid="composer-region" className={`mx-auto w-full max-w-[768px] shrink-0 px-2 min-[24rem]:px-5 ${keyboardInset > 0 ? "" : "pb-[env(safe-area-inset-bottom)]"}`}>
      <ComposerTopSlot desktopId={desktopId} sessionId={sessionId} />
      {above}
      {children}
    </div>
  );
}
