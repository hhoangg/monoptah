// What a screen inside a pane needs to know: with several panes only the focused one owns the page's h1.
import { createContext, useContext, type ReactNode } from "react";

export interface PaneInfo {
  focused: boolean;
  /** More than one pane is on screen. */
  multi: boolean;
}

export const PaneContext = createContext<PaneInfo>({ focused: true, multi: false });

/** The heading element of a screen's title: `h1` outside a split and in the focused pane, `h2` in the others. */
export function usePaneHeading(): "h1" | "h2" {
  const pane = useContext(PaneContext);
  return pane.multi && !pane.focused ? "h2" : "h1";
}

/** A screen's title: `h1` outside a split and in the focused pane, `h2` in the others. */
export function PaneHeading({ className, children }: { className?: string; children: ReactNode }) {
  return usePaneHeading() === "h1" ? <h1 className={className}>{children}</h1> : <h2 className={className}>{children}</h2>;
}
