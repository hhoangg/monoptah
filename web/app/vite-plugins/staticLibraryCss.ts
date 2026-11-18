// S-08 patch 1 (spikes/s-08-csp/FINDINGS.md, security.md section 8): Sonner and vaul call `__insertCSS("...")` at import
// time, which appends a runtime <style> element that the production CSP (`style-src 'self'`) blocks. Their stylesheets are
// bundled as normal CSS instead (ui/sonner.tsx imports sonner/dist/styles.css, ui/drawer.tsx imports styles/vaul.css),
// so this plugin only removes the runtime call. It throws when the call is not found (a library update changed its
// shape), when a library was never transformed, and when an emitted chunk still creates a <style> element at runtime.
import type { Plugin } from "vite";

/** Runtime style injection as the libraries write it: createElement(style) then `.type = text/css` (minified or not). */
export const RUNTIME_STYLE = /createElement\(["'`]style["'`]\)\s*[;,]\s*\w+\.type\s*=\s*["'`]text\/css/;
const TARGET = /node_modules\/(sonner|vaul)\/dist\/index\.mjs$/;
const INSERT_CALL = /^__insertCSS\((".*")\);?\s*$/m;
export const LIBRARIES = ["sonner", "vaul"] as const;

/** Returns the library name and the code without its `__insertCSS(...)` call; throws when the call is missing. */
export function stripInsertCss(code: string, id: string): { library: string; code: string } | undefined {
  const match = TARGET.exec(id);
  if (!match) return undefined;
  if (!INSERT_CALL.test(code)) throw new Error(`static-library-css: no __insertCSS call found in ${id}`);
  return { library: match[1] ?? "", code: code.replace(INSERT_CALL, "") };
}

export function staticLibraryCss(): Plugin {
  const seen = new Set<string>();
  return {
    name: "static-library-css",
    enforce: "pre",
    // The Worker build (the Cloudflare plugin's own environment) never bundles the UI libraries.
    applyToEnvironment: (environment) => environment.name === "client",
    buildStart() {
      seen.clear();
    },
    transform(code, id) {
      const result = stripInsertCss(code, id);
      if (!result) return undefined;
      seen.add(result.library);
      return { code: result.code, map: null };
    },
    generateBundle(_options, bundle) {
      for (const library of LIBRARIES) {
        if (!seen.has(library)) throw new Error(`static-library-css: ${library} was never transformed (its runtime style was not removed)`);
      }
      for (const [file, chunk] of Object.entries(bundle)) {
        if (chunk.type === "chunk" && RUNTIME_STYLE.test(chunk.code)) {
          throw new Error(`static-library-css: ${file} still injects a <style> element at runtime`);
        }
      }
    },
  };
}
