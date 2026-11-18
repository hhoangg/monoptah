import { Link, type LinkProps } from "@tanstack/react-router";
import { createContext, useContext, type ComponentProps, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "./Icon";

const CardHeadingLevel = createContext<"h2" | "h3">("h2");

/** Cards inside render their title as h3: wrap the cards of a page area that has its own h2. */
export function SettingsCardsUnderHeading({ children }: { children: ReactNode }) {
  return <CardHeadingLevel.Provider value="h3">{children}</CardHeadingLevel.Provider>;
}

export interface SettingsCardProps extends Omit<ComponentProps<"section">, "title"> {
  /** Group heading above the card (muted 15). */
  title?: string;
  /** Footnote under the card (muted 13/18). */
  footnote?: ReactNode;
}

/** Settings card (design-system.md section 8): bg surface, radius 22, padding 10/6, rows inside. */
export function SettingsCard({ title, footnote, className, children, ...props }: SettingsCardProps) {
  const level = useContext(CardHeadingLevel);
  const headingClass = "px-1 text-label font-medium text-muted-text";
  return (
    <section className={cn("flex flex-col gap-1.5", className)} {...props}>
      {title ? level === "h3" ? <h3 className={headingClass}>{title}</h3> : <h2 className={headingClass}>{title}</h2> : null}
      <div data-slot="settings-card" className="rounded-card bg-surface px-2.5 py-1.5">
        {children}
      </div>
      {footnote ? <p className="px-1 text-meta text-muted-text">{footnote}</p> : null}
    </section>
  );
}

export interface SettingsRowProps {
  icon?: IconName;
  title: ReactNode;
  description?: ReactNode;
  /** Right side content (a switch, a value, a button). */
  trailing?: ReactNode;
  /** Renders a button row with a chevron. */
  onClick?: () => void;
  /** Renders a router link row with a chevron; use for in-app targets. */
  to?: LinkProps["to"];
  /** Path params of `to`, for routes like /d/$desktopId/settings. */
  params?: LinkProps["params"];
  className?: string;
  /** With `onClick`: keeps the button (and keyboard focus) but ignores clicks, announced as disabled. */
  disabled?: boolean;
  /** Renders a static row as a list item, so the divider logic sees its siblings (rows in a `<ul>`). */
  as?: "li";
}

const ROW_BASE =
  "relative flex min-h-13 w-full flex-wrap items-center gap-x-3 gap-y-1 rounded-row px-1.5 py-2 text-left text-row text-content before:absolute before:top-0 before:right-0 before:h-px before:bg-stroke first:before:hidden";

/**
 * One row: min height 52, radius 12, 36px icon slot, dividers 1px inset 40 with an icon (16 without).
 * Pass `to` or `onClick` to make the whole row interactive.
 */
export function SettingsRow({ icon, title, description, trailing, onClick, to, params, className, as, disabled }: SettingsRowProps) {
  const interactive = Boolean(onClick || to);
  const classes = cn(ROW_BASE, icon ? "before:left-10" : "before:left-4", interactive && "hover:bg-selection active:opacity-60", className);
  const body = (
    <>
      {icon ? (
        <span className="inline-flex size-9 shrink-0 items-center justify-center text-muted-text">
          <Icon name={icon} size={18} />
        </span>
      ) : null}
      <span className="min-w-0 flex-[1_1_8rem] wrap-anywhere">
        <span className="block">{title}</span>
        {description ? <span className="block text-meta text-muted-text">{description}</span> : null}
      </span>
      {trailing ? <span className="shrink-0 text-label text-muted-text">{trailing}</span> : null}
      {interactive ? <Icon name="chevronRight" size={18} className="shrink-0 text-faint" /> : null}
    </>
  );
  if (to) {
    return (
      <Link to={to} params={params} className={classes}>
        {body}
      </Link>
    );
  }
  if (onClick) {
    return (
      <button type="button" onClick={disabled ? undefined : onClick} aria-disabled={disabled || undefined} className={cn(classes, disabled && "opacity-40")}>
        {body}
      </button>
    );
  }
  if (as === "li") return <li className={classes}>{body}</li>;
  return <div className={classes}>{body}</div>;
}
