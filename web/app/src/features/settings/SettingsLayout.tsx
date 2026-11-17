import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * Page column of the settings screens (account and per-desktop). Layout choice: a single centered
 * column, max 760px, with the gutters growing by the safe-area insets, on every width. Design-system
 * 11 allows two-pane on tablet and desktop, but the groups here are short, so a section list beside
 * them would add a click and no clarity; one column also keeps 200% zoom and 390px identical.
 */
export function SettingsLayout({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "mx-auto flex w-full max-w-[760px] flex-col gap-6 px-[max(1.25rem,env(safe-area-inset-left))] pt-6 pr-[max(1.25rem,env(safe-area-inset-right))] pb-4 sm:pt-10",
        className,
      )}
    >
      {children}
    </div>
  );
}
