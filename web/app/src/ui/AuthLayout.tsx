import type { ReactNode } from "react";
import { cn } from "./cn";
import { Glass } from "./Glass";

/**
 * Centered column for the sign-in family of routes: max 420px, 20px gutters that grow with the
 * safe-area insets, content on one glass card.
 */
export function AuthLayout({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className="mx-auto flex w-full max-w-[420px] flex-col px-[max(1.25rem,env(safe-area-inset-left))] pt-6 pr-[max(1.25rem,env(safe-area-inset-right))] sm:pt-12">
      <Glass className={cn("flex flex-col gap-5 rounded-card! p-5", className)}>{children}</Glass>
    </div>
  );
}
