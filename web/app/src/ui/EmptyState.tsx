import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "./Icon";

export interface EmptyStateProps {
  /** The mark above the headline, drawn at 30% opacity. */
  icon: IconName;
  /** The headline: says what is missing. */
  title: string;
  /** The caption: says what to do next. */
  description?: ReactNode;
  /** An optional next step, usually a button or link. */
  action?: ReactNode;
  /** The heading element of the title. */
  titleAs?: "h2" | "h3";
  className?: string;
  "data-testid"?: string;
}

/** Empty state (design-system 12.18): a mark, a headline, a caption and an optional action; never a bare grey line. */
export function EmptyState({ icon, title, description, action, titleAs: Title = "h2", className, "data-testid": testId }: EmptyStateProps) {
  return (
    <section data-testid={testId} className={cn("flex flex-col items-center gap-3 px-4 py-10 text-center", className)}>
      <Icon name={icon} size={40} className="opacity-30" />
      <Title className="text-title font-semibold">{title}</Title>
      {description ? <p className="max-w-[40ch] text-body text-muted-text">{description}</p> : null}
      {action ? <div className="pt-1">{action}</div> : null}
    </section>
  );
}
