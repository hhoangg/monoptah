import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { PageHeader } from "@/ui/PageHeader";

const TABS = [
  { to: "/admin", label: "Overview", exact: true },
  { to: "/admin/users", label: "Users", exact: false },
  { to: "/admin/invites", label: "Invites", exact: false },
] as const;

export interface AdminLayoutProps {
  title: string;
  description?: ReactNode;
  /** The users screen widens its content (not the header and tabs) for its list and detail panel. */
  wide?: boolean;
  children: ReactNode;
}

/** Page column of the admin screens: the route's h1, a small section nav, then the content. */
export function AdminLayout({ title, description, wide = false, children }: AdminLayoutProps) {
  return (
    <div className="mx-auto flex w-full max-w-[1200px] flex-col gap-5 px-[max(1.25rem,env(safe-area-inset-left))] pt-6 pr-[max(1.25rem,env(safe-area-inset-right))] pb-4 sm:pt-10">
      <div className="flex max-w-[760px] flex-col gap-5">
        <PageHeader title={title} description={description} />
        <nav aria-label="Admin" className="flex flex-wrap gap-1.5">
          {TABS.map((tab) => (
            <Link
              key={tab.to}
              to={tab.to}
              activeOptions={{ exact: tab.exact }}
              className="inline-flex min-h-11 items-center rounded-pill px-4 text-small font-medium text-muted-text hover:bg-selection pointer-fine:min-h-9 [&.active]:bg-fill [&.active]:text-content"
            >
              {tab.label}
            </Link>
          ))}
        </nav>
      </div>
      <div className={cn("flex w-full flex-col gap-5", !wide && "max-w-[760px]")}>{children}</div>
    </div>
  );
}
