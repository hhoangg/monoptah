// Fork-local (Monoptah). The Host version is decoupled from the app and comes
// from host/VERSION. host/build.mjs and host/vitest.config.ts inject it as
// __HOST_VERSION__ (for example "1.0.0+monoptah").
declare const __HOST_VERSION__: string;

/** Full Host version, including the "+monoptah" flavor marker. */
export const HOST_VERSION: string = __HOST_VERSION__;
/** Flavor reported by environment.describe. */
export const HOST_FLAVOR = "monoptah";
/** Version without semver build metadata, as reported by environment.describe. */
export const HOST_VERSION_BASE: string = HOST_VERSION.split("+")[0];
