import { useEffect, useState, type MouseEvent, type ReactNode } from "react";
import { cn } from "@/lib/utils";

/** One entry of the section index of a long settings page: an anchor to the element with this `id`. */
export interface SectionLink {
  id: string;
  label: string;
}

/**
 * Page column of the settings screens (account and per-desktop): a centered column, max 760px, with the
 * gutters growing by the safe-area insets. A page that passes `sections` (the account settings) also gets
 * a sticky index beside the column from 1024px up (design-system 11: two panes on tablet and desktop);
 * below that, the page shows `SectionPills` itself, under its header.
 */
export function SettingsLayout({ children, className, sections }: { children: ReactNode; className?: string; sections?: readonly SectionLink[] }) {
  const gutters = "px-[max(1.25rem,env(safe-area-inset-left))] pr-[max(1.25rem,env(safe-area-inset-right))]";
  if (!sections) {
    return <div className={cn("mx-auto flex w-full max-w-[760px] flex-col gap-6 pt-6 pb-4 sm:pt-10", gutters, className)}>{children}</div>;
  }
  return (
    <div className={cn("mx-auto w-full max-w-[760px] pt-6 pb-4 sm:pt-10 lg:grid lg:max-w-[1040px] lg:grid-cols-[11rem_minmax(0,760px)] lg:gap-10", gutters)}>
      <aside className="hidden lg:block">
        <div className="sticky top-10">
          <SectionIndex sections={sections} orientation="column" />
        </div>
      </aside>
      <div className={cn("flex min-w-0 flex-col gap-6", className)}>{children}</div>
    </div>
  );
}

/** The id of the section nearest the top of the viewport; the first one until the page scrolls. */
function useActiveSection(ids: readonly string[]): [string | undefined, (id: string) => void] {
  const [active, setActive] = useState<string | undefined>(ids[0]);
  const key = ids.join("|");
  useEffect(() => {
    if (typeof IntersectionObserver === "undefined") return undefined;
    const visible = new Set<string>();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) visible.add(entry.target.id);
          else visible.delete(entry.target.id);
        }
        const first = key.split("|").find((id) => visible.has(id));
        if (first) setActive(first);
      },
      { rootMargin: "0px 0px -60% 0px" },
    );
    for (const id of key.split("|")) {
      const node = document.getElementById(id);
      if (node) observer.observe(node);
    }
    return () => observer.disconnect();
  }, [key]);
  return [active, setActive];
}

function jumpTo(event: MouseEvent<HTMLAnchorElement>, id: string, setActive: (id: string) => void) {
  const node = document.getElementById(id);
  if (!node) return;
  event.preventDefault();
  setActive(id);
  const reduced = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  node.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
}

function SectionIndex({ sections, orientation }: { sections: readonly SectionLink[]; orientation: "column" | "row" }) {
  const [active, setActive] = useActiveSection(sections.map((section) => section.id));
  return (
    <nav aria-label="Settings sections" className={cn("flex gap-1.5", orientation === "column" ? "flex-col" : "flex-wrap")}>
      {sections.map((section) => (
        <a
          key={section.id}
          href={`#${section.id}`}
          aria-current={active === section.id ? "true" : undefined}
          onClick={(event) => jumpTo(event, section.id, setActive)}
          className={cn(
            "inline-flex min-h-11 items-center px-4 text-small font-medium text-muted-text hover:bg-selection pointer-fine:min-h-9 aria-[current=true]:bg-fill aria-[current=true]:text-content",
            orientation === "column" ? "rounded-row" : "rounded-pill",
          )}
        >
          {section.label}
        </a>
      ))}
    </nav>
  );
}

/** The section index as a wrapping pill row, for widths under 1024px (the sticky column takes over from there). */
export function SectionPills({ sections }: { sections: readonly SectionLink[] }) {
  return (
    <div className="lg:hidden">
      <SectionIndex sections={sections} orientation="row" />
    </div>
  );
}
