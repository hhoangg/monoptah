import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * Page column of the standalone settings-style screens (the public install guide, desktop key verification): a centered
 * column, max 760px, with the gutters growing by the safe-area insets. The pages of the settings shell use `SettingsShell`
 * instead, which adds the nav column.
 */
export function SettingsLayout({ children, className }: { children: ReactNode; className?: string }) {
  const gutters = "px-[max(1.25rem,env(safe-area-inset-left))] pr-[max(1.25rem,env(safe-area-inset-right))]";
  return <div className={cn("mx-auto flex w-full max-w-[760px] flex-col gap-6 pt-6 pb-[calc(env(safe-area-inset-bottom)+1rem)] sm:pt-10", gutters, className)}>{children}</div>;
}
