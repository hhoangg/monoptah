// The frame of the Mono sub pages (profile, documents, habit, file, edit): a back link, the page's only h1 and
// optional actions over a centred column, the way the session screen frames its header.
import type { ReactNode } from "react";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";

export function MonoPage({ back, title, actions, children, testId }: { back: ReactNode; title: string; actions?: ReactNode; children: ReactNode; testId?: string }) {
  return (
    <div className="mx-auto flex w-full max-w-[768px] flex-col gap-4 px-3 pb-10" data-testid={testId}>
      <header className="flex items-center gap-2 py-2">
        {back}
        <h1 className="min-w-0 flex-1 truncate text-headline font-semibold">{title}</h1>
        {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
      </header>
      {children}
    </div>
  );
}

/** The round back button class shared by the Mono pages. */
export const BACK_LINK_CLASS = "inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection pointer-fine:size-9";

export function BackIcon() {
  return <Icon name="chevronDown" size={20} className="rotate-90" />;
}

/** The refresh button that replaces pull-to-refresh (F-76, F-77). */
export function RefreshButton({ onRefresh, refreshing }: { onRefresh: () => void; refreshing: boolean }) {
  return (
    <Button type="button" variant="circle" aria-label="Refresh" disabled={refreshing} aria-busy={refreshing} onClick={onRefresh}>
      <Icon name="refresh" size={18} className={refreshing ? "animate-spin" : undefined} />
    </Button>
  );
}

/** A titled group of the profile pages. */
export function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h2 className="px-1 text-label font-medium text-muted-text">{title}</h2>
      <div className="rounded-card bg-surface px-2.5 py-1.5">{children}</div>
    </section>
  );
}
