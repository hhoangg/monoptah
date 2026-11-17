// Generates `_headers` for the Worker assets directory (security.md 8.1). The CSP comes from the
// Worker's own `buildCsp`, so the policy exists in one place; only the host is decided here.
import { readFileSync } from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";
import { buildCsp, COMPANION_HEADERS } from "../../worker/src/csp.ts";

/** Env override for self-hosters whose origin differs from the one in `wrangler.jsonc` `vars`. */
export const RP_ORIGIN_ENV = "MONOPTAH_RP_ORIGIN";

/** Removes `//` and `/* *\/` comments and trailing commas outside strings, so JSON.parse can read a .jsonc file. */
export function stripJsonc(text: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const ch = text[i] as string;
    const next = text[i + 1];
    if (ch === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
    } else if (ch === "/" && next === "/") {
      while (i < text.length && text[i] !== "\n") i++;
    } else if (ch === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end === -1 ? text.length : end + 2;
    } else {
      out += ch;
      i++;
    }
  }
  return out.replace(/,(\s*[}\]])/g, "$1");
}

/** `RP_ORIGIN` from `wrangler.jsonc` `vars`, unless the env override is set. */
export function resolveRpOrigin(wranglerPath: string, env: Record<string, string | undefined> = process.env): string {
  const override = env[RP_ORIGIN_ENV];
  if (override) return override;
  const config = JSON.parse(stripJsonc(readFileSync(wranglerPath, "utf8"))) as { vars?: { RP_ORIGIN?: string } };
  const origin = config.vars?.RP_ORIGIN;
  if (!origin) throw new Error(`RP_ORIGIN is not set in ${wranglerPath} vars (or ${RP_ORIGIN_ENV})`);
  return origin;
}

/** The text of `_headers` for an RP origin such as `https://zptah.app`. */
export function renderHeaders(rpOrigin: string): string {
  const host = new URL(rpOrigin).host;
  const companion = Object.entries(COMPANION_HEADERS).map(([name, value]) => `  ${name}: ${value}`);
  return [
    "# Generated at build time by web/app/vite-plugins/headers.ts. Never edit by hand (security.md 8.1).",
    "# The Worker adds the same headers on HTML it serves itself and on /api/*.",
    "/*",
    `  Content-Security-Policy: ${buildCsp(host)}`,
    ...companion,
    "  Cache-Control: no-cache",
    "",
    "# Vite emits content-hashed files under /assets: cache them for a year. Matching rules are merged,",
    "# so `! Cache-Control` first removes the `no-cache` value set by the /* rule.",
    "/assets/*",
    "  ! Cache-Control",
    "  Cache-Control: public, max-age=31536000, immutable",
    "",
  ].join("\n");
}

/** Vite plugin: emits `_headers` into the client output of a build. */
export function headersPlugin(wranglerPath: string): Plugin {
  return {
    name: "monoptah-headers",
    apply: "build",
    generateBundle() {
      // The Cloudflare plugin also builds a Worker environment; its output must not get the file.
      if (this.environment.name !== "client") return;
      this.emitFile({ type: "asset", fileName: "_headers", source: renderHeaders(resolveRpOrigin(path.resolve(wranglerPath))) });
    },
  };
}
