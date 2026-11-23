import type { LinkProps } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Badge } from "@/ui/badge";
import { PageHeader } from "@/ui/PageHeader";
import { BROWSER_ONLY_LABEL } from "./settingsIndex";

export interface SettingsPageProps {
  /** The page's only h1. */
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  /**
   * The back link. Below 1024px the settings list is a page of its own, so every sub page needs a way back; from
   * 1024px the nav sits beside the page and the link is hidden. Defaults to the settings list.
   */
  back?: { to: LinkProps["to"]; params?: LinkProps["params"]; label: string };
  /** Adds the "Only on this browser" badge under the title (the pages of the This browser group). */
  browserOnly?: boolean;
  className?: string;
  children?: ReactNode;
}

/** One page of the settings shell (route /settings/*): the header with its back link, then the cards. */
export function SettingsPage({ title, description, actions, back, browserOnly, className, children }: SettingsPageProps) {
  const target = back ?? { to: "/settings", label: "Settings" };
  return (
    <div className={cn("flex min-w-0 flex-col gap-6", className)}>
      <PageHeader
        title={title}
        description={
          browserOnly ? (
            <span className="flex flex-col items-start gap-1.5">
              {description ? <span>{description}</span> : null}
              <Badge variant="outline">{BROWSER_ONLY_LABEL}</Badge>
            </span>
          ) : (
            description
          )
        }
        actions={actions}
        back={{ to: target.to, params: target.params, label: target.label, className: "lg:hidden" }}
      />
      {children}
    </div>
  );
}
