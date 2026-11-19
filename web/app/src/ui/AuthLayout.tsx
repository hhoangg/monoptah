import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Glass } from "./Glass";

/**
 * Centered column for the sign-in family of routes: max 420px. From 640px the content sits on one glass card
 * (a `sheet` tint, so inner `surface` panels stay visible) with 20px gutters that grow with the safe-area insets.
 * Below that the card is dropped: content is full-bleed with a 28px gutter, like the native sign-in.
 */
export function AuthLayout({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className="mx-auto flex w-full max-w-[420px] flex-col pt-6 pr-[max(1.75rem,env(safe-area-inset-right))] pl-[max(1.75rem,env(safe-area-inset-left))] sm:pt-12 sm:pr-[max(1.25rem,env(safe-area-inset-right))] sm:pl-[max(1.25rem,env(safe-area-inset-left))]">
      <Glass
        tint="var(--sheet)"
        className={cn(
          "flex flex-col gap-5 rounded-card p-5 max-sm:bg-transparent max-sm:p-0 max-sm:backdrop-filter-none max-sm:before:hidden",
          className,
        )}
      >
        {children}
      </Glass>
    </div>
  );
}
