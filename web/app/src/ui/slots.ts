// Slots registry (workstreams.md 3.2, "Web slots in the session screens").
//
// A screen owner declares a place with <Slot name="...">; a later workstream fills it from its
// `features/<name>/register.ts` with `registerSlot(name, Component)`. A missing registration renders
// the default: nothing, except `app-shell`, which passes its children through. Registrations happen
// once at startup (main.tsx imports every register.ts), before the first render, so the registry
// is a plain map and not reactive.
//
// One component per slot name: registering a name twice replaces the earlier registration.
import { createElement, Fragment, type ComponentType, type ReactElement, type ReactNode } from "react";

export const SLOT_NAMES = [
  "composer-top",
  "session-card-badge",
  "session-header-chip",
  "session-menu-items",
  "archive-dialog",
  "send-guard",
  "new-session-target",
  "desktop-nav-items",
  "session-capabilities",
  "desktop-settings-sections",
  "desktop-switcher",
  "link-state-screens",
  "first-run-picker",
  "app-shell",
  "settings-sections",
] as const;

export type SlotName = (typeof SLOT_NAMES)[number];

/** Props every slot accepts; a slot owner narrows its own entry in `SlotPropsMap`. */
export interface SlotBaseProps {
  children?: ReactNode;
  [prop: string]: unknown;
}

/**
 * Props per slot. Wave 0 declares every slot with the open base props; the owner of a slot narrows
 * its entry (for example `new-session-target` takes `value` and `onChange(target | null)`).
 */
export interface SlotPropsMap extends Record<SlotName, SlotBaseProps> {
  "new-session-target": SlotBaseProps & { value?: unknown; onChange?: (target: unknown) => void };
}

const registry = new Map<SlotName, ComponentType<never>>();

export function registerSlot<N extends SlotName>(name: N, component: ComponentType<SlotPropsMap[N]>): void {
  registry.set(name, component as ComponentType<never>);
}

export function getSlot<N extends SlotName>(name: N): ComponentType<SlotPropsMap[N]> | undefined {
  return registry.get(name) as ComponentType<SlotPropsMap[N]> | undefined;
}

/** Drops every registration. For tests. */
export function clearSlots(): void {
  registry.clear();
}

/** Slots whose default renders the children instead of nothing. */
const PASSTHROUGH: ReadonlySet<SlotName> = new Set<SlotName>(["app-shell"]);

/** Renders the slot's registered component, else the default (nothing, or the children for `app-shell`). */
export function renderSlot<N extends SlotName>(name: N, props: SlotPropsMap[N]): ReactElement | null {
  const component = getSlot(name);
  if (component) return createElement(component as ComponentType<SlotBaseProps>, props);
  if (PASSTHROUGH.has(name)) return createElement(Fragment, null, props.children);
  return null;
}

/** `<Slot name="composer-top" {...props}>fallback children</Slot>`. */
export function Slot<N extends SlotName>({ name, ...props }: { name: N } & SlotPropsMap[N]): ReactElement | null {
  return renderSlot(name, props as unknown as SlotPropsMap[N]);
}
