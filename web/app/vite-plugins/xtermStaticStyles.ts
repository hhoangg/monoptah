// The terminal (security.md "Terminal", 8.1): xterm.js creates three <style> elements at run time (its theme, its cell size
// and its scrollbar), which the production CSP (`style-src 'self'`) blocks. This plugin rewrites those three
// `createElement("style")` calls in the installed xterm bundle to `createSheetStyle()` (src/features/terminal/sheetStyle.ts),
// which keeps the CSS in an adopted constructed stylesheet instead. No CSP change is needed. It throws when the bundle no
// longer has exactly three such calls (an xterm update changed its shape) and when an emitted chunk still builds a <style>
// element out of xterm code, so an update fails the build instead of shipping a broken terminal.
import path from "node:path";
import type { Plugin } from "vite";

const TARGET = /\/@xterm\/xterm\/lib\/xterm\.mjs$/;
/** `this._document.createElement("style")` and `<x>.mainDocument.createElement("style")`, as the minified bundle spells them. */
const STYLE_CALL = /(?:this\._document|[\w$]+\.mainDocument)\.createElement\("style"\)/g;
export const EXPECTED_STYLE_CALLS = 3;
const XTERM_CHUNK = /xterm-dom-renderer-owner/;
const STYLE_ELEMENT = /createElement\(["'`]style["'`]\)/g;
const ALLOWED_STYLE_ELEMENTS = 1;
const SHEET_STYLE = path.resolve(import.meta.dirname, "../src/features/terminal/sheetStyle.ts").replaceAll("\\", "/");

/** Returns the rewritten code, or undefined for a module that is not the xterm bundle. Throws when the shape changed. */
export function replaceStyleElements(code: string, id: string): string | undefined {
  if (!TARGET.test(id.replaceAll("\\", "/"))) return undefined;
  const found = code.match(STYLE_CALL)?.length ?? 0;
  if (found !== EXPECTED_STYLE_CALLS) {
    throw new Error(`xterm-static-styles: expected ${EXPECTED_STYLE_CALLS} runtime <style> calls in ${id}, found ${found}`);
  }
  return `import { createSheetStyle as __xtermSheetStyle } from ${JSON.stringify(SHEET_STYLE)};\n${code.replace(STYLE_CALL, "__xtermSheetStyle()")}`;
}

export function xtermStaticStyles(): Plugin {
  let seen = false;
  return {
    name: "xterm-static-styles",
    enforce: "pre",
    applyToEnvironment: (environment) => environment.name === "client",
    buildStart() {
      seen = false;
    },
    transform(code, id) {
      const result = replaceStyleElements(code, id);
      if (result === undefined) return undefined;
      seen = true;
      return { code: result, map: null };
    },
    generateBundle(_options, bundle) {
      // The terminal chunk is only built when a route imports xterm; a build without it has nothing to check.
      if (!seen) return;
      let checked = false;
      for (const [file, chunk] of Object.entries(bundle)) {
        if (chunk.type !== "chunk" || !XTERM_CHUNK.test(chunk.code)) continue;
        checked = true;
        // The one left is the fallback of sheetStyle.ts for a browser without adoptable stylesheets.
        const left = chunk.code.match(STYLE_ELEMENT)?.length ?? 0;
        if (left !== ALLOWED_STYLE_ELEMENTS) throw new Error(`xterm-static-styles: ${file} creates ${left} <style> elements at run time, expected ${ALLOWED_STYLE_ELEMENTS}`);
      }
      if (!checked) throw new Error("xterm-static-styles: the xterm chunk was not found in the bundle");
    },
  };
}
