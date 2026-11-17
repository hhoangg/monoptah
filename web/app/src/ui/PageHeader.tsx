import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export interface PageHeaderProps {
  /** The route's only h1. */
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  /** `display` (34/40) for the sign-in title; `headline` (22/28) everywhere else. */
  size?: "display" | "headline";
  className?: string;
}

export function PageHeader({ title, description, actions, size = "headline", className }: PageHeaderProps) {
  return (
    <header className={cn("flex items-start justify-between gap-3", className)}>
      <div className="min-w-0 flex-1">
        <h1 className={cn("font-semibold text-content", size === "display" ? "text-display" : "text-headline")}>{title}</h1>
        {description ? <p className="mt-1 text-body text-muted-text">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  );
}
