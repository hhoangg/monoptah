import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon } from "./Icon";

export interface NoticeProps {
  /** `error` is announced at once (`role="alert"`); `info` is a polite status. */
  variant?: "error" | "info";
  title?: string;
  children: ReactNode;
  className?: string;
}

/** Notice (design-system.md section 8): bg content-a04, 1px stroke, radius 16, padding 14, icon plus text. */
export function Notice({ variant = "info", title, children, className }: NoticeProps) {
  const isError = variant === "error";
  return (
    <div
      role={isError ? "alert" : "status"}
      data-variant={variant}
      className={cn("flex items-start gap-2.5 rounded-[16px] border border-stroke bg-content-a04 p-3.5 text-small text-content", className)}
    >
      <Icon name={isError ? "alert" : "info"} size={18} className={cn("mt-0.5 shrink-0", isError ? "text-deletions" : "text-muted-text")} />
      <div className="min-w-0 flex-1">
        {title ? <p className="font-semibold">{title}</p> : null}
        <div className={title ? "text-muted-text" : undefined}>{children}</div>
      </div>
    </div>
  );
}
