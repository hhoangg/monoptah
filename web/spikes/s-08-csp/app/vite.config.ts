// Build of the S-08 stub. `SPIKE_RAW=1` skips the CSS patch plugin (shows the unpatched violations).
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const raw = process.env["SPIKE_RAW"] === "1";
const root = fileURLToPath(new URL(".", import.meta.url));

/**
 * Sonner and vaul call `__insertCSS("...")` at import time, which appends a runtime `<style>`
 * element (blocked by `style-src 'self'`). This plugin removes the call and serves the very same
 * string as a CSS module, so Vite emits it as a normal same-origin stylesheet.
 */
function staticLibraryCss(): Plugin {
  const re = /^__insertCSS\((".*")\);?\s*$/m;
  const target = /node_modules\/(sonner|vaul)\/dist\/index\.mjs$/;
  const css = new Map<string, string>();
  return {
    name: "static-library-css",
    enforce: "pre",
    resolveId(id) {
      return id.startsWith("\0static-css:") ? id : undefined;
    },
    load(id) {
      if (!id.startsWith("\0static-css:")) return undefined;
      const text = css.get(id.slice("\0static-css:".length));
      return text === undefined ? undefined : text;
    },
    transform(code, id) {
      const m = target.exec(id);
      if (!m) return undefined;
      const hit = re.exec(code);
      if (!hit) throw new Error(`static-library-css: no __insertCSS call found in ${id}`);
      const name = `${m[1]}.css`;
      // vaul ships one invalid rule (`[data-vaul-handle-hitarea]:{...}`) that browsers drop when parsing;
      // the CSS minifier rejects it, so remove it (same behavior as the runtime-injected string).
      const text = (JSON.parse(hit[1]!) as string).replace(/@media \(pointer:fine\)\{\[data-vaul-handle-hitarea\]:\{[^}]*\}\}/, "");
      css.set(name, text);
      return code.replace(re, `import "\0static-css:${name}";`);
    },
  };
}

export default defineConfig({
  root,
  base: "/",
  plugins: [react(), ...(raw ? [] : [staticLibraryCss()])],
  resolve: raw ? {} : { alias: [{ find: /^react-style-singleton$/, replacement: `${root}src/patches/style-singleton.ts` }] },
  build: {
    outDir: raw ? "dist/raw" : "dist/patched",
    emptyOutDir: true,
    modulePreload: { polyfill: true },
    rollupOptions: {
      input: { main: `${root}index.html`, sw: `${root}src/sw.ts` },
      output: {
        entryFileNames: (chunk) => (chunk.name === "sw" ? "sw.js" : "assets/[name]-[hash].js"),
      },
    },
  },
});
