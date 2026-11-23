import { Link, type LinkProps } from "@tanstack/react-router";
import { createContext, useContext, type ComponentProps, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "./Icon";

/**
 * Shared class strings of the grouped-row look (design-system.md section 8). Tailwind only sees literal strings, so the
 * variants are written out in full here and reused by every card that renders its own rows.
 *
 * Geometry: the card has the same inset on all four sides (`--settings-card-inset`, tailwind.css) and a row highlight has
 * the radius of the card minus that inset (`rounded-row`), so the two curves are concentric. A divider is the 1px top
 * edge of a row: it starts where the text starts (row padding 6 + icon slot 36 + gap 12 = 54px with an icon, 6px without)
 * and ends where the text ends (6px). It is never drawn through a highlighted row: the row's own divider and the divider
 * of the row after it fade out while the row is hovered, keyboard focused or the current page.
 */
/**
 * Scroll margin of an anchor target on a settings page, so `scrollIntoView` and a `#hash` jump stop clear of what can
 * cover the top: nothing but the connection banner pill (tailwind.css, `--settings-anchor-offset`).
 */
export const SETTINGS_ANCHOR = "scroll-mt-[var(--settings-anchor-offset)]";

export const SETTINGS_CARD_BODY = "rounded-card bg-surface p-[var(--settings-card-inset)]";

/** A row of a card: min height 52, radius 12, 6px side padding. */
export const SETTINGS_ROW_BASE = "relative flex min-h-13 w-full flex-wrap items-center gap-x-3 gap-y-1 rounded-row px-1.5 py-2 text-left text-row text-content";

// "Highlighted" is hovered, keyboard focused or the current page, and only the element that draws the highlight counts: it
// carries the `data-row-highlight` attribute (a link or button next to it, such as "Manage", does not). The row after a
// highlighted one hides its divider too, whether the rows are siblings (the highlighted `a`/`button` rows side by side) or
// `<li>` wrappers that hold the marked link or button.
// The hover part sits in `@media (hover: hover)` like Tailwind's own `hover:`, so a tap on a touch screen (a sticky :hover
// with no highlight) does not hide the neighbour's divider. Focus-visible and the current page stay unconditional.
const AFTER_HOVERED =
  "[@media(hover:hover)]:[[data-row-highlight]:hover+&]:before:opacity-0 [@media(hover:hover)]:[:has(>[data-row-highlight]:hover)+&]:before:opacity-0";
const AFTER_FOCUSED_OR_CURRENT =
  "[[data-row-highlight]:is(:focus-visible,[aria-current=page])+&]:before:opacity-0 [:has(>[data-row-highlight]:is(:focus-visible,[aria-current=page]))+&]:before:opacity-0";
const AFTER_HIGHLIGHTED = `${AFTER_HOVERED} ${AFTER_FOCUSED_OR_CURRENT}`;
/** The divider without a start offset: add a `before:left-*` class, for a row whose text starts elsewhere. */
export const SETTINGS_DIVIDER_BASE = `before:absolute before:top-0 before:right-1.5 before:h-px before:bg-stroke first:before:hidden ${AFTER_HIGHLIGHTED}`;
/** Divider of a row that starts with a 36px icon slot. */
export const SETTINGS_DIVIDER_ICON = `${SETTINGS_DIVIDER_BASE} before:left-[54px]`;
/** Divider of a row whose text starts at the row padding. */
export const SETTINGS_DIVIDER_PLAIN = `${SETTINGS_DIVIDER_BASE} before:left-1.5`;

/** An interactive row that is itself the `a` or `button`: its own divider fades out while it is highlighted. */
export const SETTINGS_HIDE_DIVIDER_SELF = "hover:before:opacity-0 focus-visible:before:opacity-0 aria-[current=page]:before:opacity-0";

/** A wrapper (`<li>`) around the marked `a` or `button`: the same, looking at the element inside. */
export const SETTINGS_HIDE_DIVIDER_SELF_WRAPPED =
  "[@media(hover:hover)]:has-[>[data-row-highlight]:hover]:before:opacity-0 has-[>[data-row-highlight]:is(:focus-visible,[aria-current=page])]:before:opacity-0";

/** The nav of the settings list page below 1024px: the same card, rows and dividers, only below lg (the sidebar has none). */
// `settings-nav-card` lets tailwind.css give the card and its rows the squircle corner below lg, like `.rounded-card`.
export const SETTINGS_NAV_CARD_BODY = "settings-nav-card max-lg:rounded-card max-lg:bg-surface max-lg:p-[var(--settings-card-inset)]";
const NAV_DIVIDER = `max-lg:before:absolute max-lg:before:top-0 max-lg:before:right-1.5 max-lg:before:h-px max-lg:before:bg-stroke max-lg:first:before:hidden ${AFTER_HIGHLIGHTED}`;
export const SETTINGS_NAV_DIVIDER_ICON = `${NAV_DIVIDER} max-lg:before:left-[54px]`;
export const SETTINGS_NAV_DIVIDER_PLAIN = `${NAV_DIVIDER} max-lg:before:left-1.5`;

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

/** Settings card (design-system.md section 8): bg surface, radius 22, the same 10px inset on every side, rows inside. */
export function SettingsCard({ title, footnote, className, children, ...props }: SettingsCardProps) {
  const level = useContext(CardHeadingLevel);
  const headingClass = "px-1 text-label font-medium text-muted-text";
  return (
    <section className={cn("flex flex-col gap-1.5", className)} {...props}>
      {title ? level === "h3" ? <h3 className={headingClass}>{title}</h3> : <h2 className={headingClass}>{title}</h2> : null}
      <div data-slot="settings-card" className={SETTINGS_CARD_BODY}>
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
  /** Path params of `to`, for routes like /settings/desktops/$id. */
  params?: LinkProps["params"];
  className?: string;
  /** With `onClick`: keeps the button (and keyboard focus) but ignores clicks, announced as disabled. */
  disabled?: boolean;
  /** Renders a static row as a list item, so the divider logic sees its siblings (rows in a `<ul>`). */
  as?: "li";
  /** DOM id of the row: the target of a settings quick-find link (`route#id`). */
  id?: string;
}

/**
 * One row: min height 52, radius 12, 36px icon slot, 1px divider from the text start (54px with an icon, 6px without)
 * to the text end. Pass `to` or `onClick` to make the whole row interactive; the dividers around its highlight fade out.
 */
export function SettingsRow({ icon, title, description, trailing, onClick, to, params, className, as, disabled, id }: SettingsRowProps) {
  const interactive = Boolean(onClick || to);
  const classes = cn(SETTINGS_ROW_BASE, icon ? SETTINGS_DIVIDER_ICON : SETTINGS_DIVIDER_PLAIN, interactive && ["hover:bg-selection active:opacity-60", SETTINGS_HIDE_DIVIDER_SELF], className);
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
      <Link to={to} params={params} id={id} data-row-highlight="" className={classes}>
        {body}
      </Link>
    );
  }
  if (onClick) {
    return (
      <button type="button" id={id} data-row-highlight="" onClick={disabled ? undefined : onClick} aria-disabled={disabled || undefined} className={cn(classes, disabled && "opacity-40")}>
        {body}
      </button>
    );
  }
  if (as === "li") return <li id={id} className={classes}>{body}</li>;
  return (
    <div id={id} className={classes}>
      {body}
    </div>
  );
}
