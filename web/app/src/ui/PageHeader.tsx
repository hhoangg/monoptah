import { Link, type LinkProps } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon } from "./Icon";

export interface PageHeaderProps {
  /** The route's only h1. */
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  /** `display` (34/40) for the sign-in title; `headline` (22/28) everywhere else. */
  size?: "display" | "headline";
  className?: string;
  /** Anything that goes where `back` goes (for example the settings exit button); `back` wins when both are given. */
  leading?: ReactNode;
  /** A 44 px circle back link before the title (design-system 3.1: an installed PWA has no browser back). */
  back?: { to: LinkProps["to"]; params?: LinkProps["params"]; label?: string; /** Tailwind classes to hide it on wide screens. */ className?: string };
}

export function PageHeader({ title, description, actions, size = "headline", className, back, leading }: PageHeaderProps) {
  return (
    <header className={cn("flex justify-between gap-3", description ? "items-start" : "items-center", className)}>
      {back ? (
        <Link
          to={back.to}
          params={back.params}
          aria-label={back.label ?? "Back"}
          className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection", back.className)}
        >
          <Icon name="chevronDown" size={20} className="rotate-90" />
        </Link>
      ) : (
        leading ?? null
      )}
      <div className="min-w-0 flex-1">
        <h1 className={cn("wrap-anywhere font-semibold text-content", size === "display" ? "text-display" : "text-headline")}>{title}</h1>
        {description ? <p className="mt-1 text-body text-muted-text">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  );
}
