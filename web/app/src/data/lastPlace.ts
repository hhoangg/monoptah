// Registry of the "last place" resolver used by `/` (W-22 and W-06). WS-T registers the real one
// (features/sessions/lastPlace.ts) later; until then there is no last place and `/` goes to /desktops.

/** Returns the path of the last place of the last used desktop (for example `/d/dk1/s/s9`), or null. */
export type LastPlaceResolver = () => string | null;

const noLastPlace: LastPlaceResolver = () => null;

let resolver: LastPlaceResolver = noLastPlace;

/** Registers the resolver; returns the previous one so a test can restore it. Pass nothing to reset. */
export function registerLastPlaceResolver(fn?: LastPlaceResolver): LastPlaceResolver {
  const previous = resolver;
  resolver = fn ?? noLastPlace;
  return previous;
}

/** The last place, or null when none is known or the resolver throws. */
export function resolveLastPlace(): string | null {
  try {
    const place = resolver();
    return typeof place === "string" && place.startsWith("/") && !place.startsWith("//") ? place : null;
  } catch {
    return null;
  }
}
