// Fixtures of `chat.mentionedFile`: the files an assistant message of the mock conversations mentions, and the
// pager and the rules the desktop applies (mention check first, deny list, asset rules). Everything is fake.
// The data has no imports from fixtures.ts so the world can seed its transcript from it.
import type { DesktopErrorCode, ResultOf } from "@monoptah/protocol";

export type MentionedFileResult = ResultOf<"chat.mentionedFile">;

/** The mock pages by this many raw bytes (the desktop uses 4 MiB), so the small fixtures exercise paging. */
export const MOCK_PAGE_BYTES = 8 * 1024;

/** The folder the fixture project (proj-alpha) lives in. */
export const MENTION_ROOT = "/example/alpha";

const encoder = new TextEncoder();

/** A 1 x 1 red PNG. */
export const PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

const SVG_TEXT =
  '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="#4f8cff"/><circle cx="32" cy="32" r="16" fill="#fff"/></svg>';

/** The sanitized document the desktop produces for `report.html`: no script, assets as `monoptah-asset:<i>`. */
export const REPORT_HTML =
  '<!doctype html><html><head><title>Example report</title></head><body style="font-family:sans-serif;margin:16px">' +
  '<h1 id="top" style="color:#c33">Example report</h1><p>Summary of the example run.</p>' +
  '<img src="monoptah-asset:0" alt="chart" style="max-width:100%">' +
  '<div style="height:24px;background:url(monoptah-asset:0)"></div>' +
  '<a href="#top">Back to top</a></body></html>';

export const FINDINGS_MD =
  "# Example findings\n\nThe chart below comes from a relative path.\n\n![chart](img/chart.png)\n\n![remote](https://example.invalid/tracker.png)\n\n- First finding\n- Second finding\n";

const LONG_LINE = "Example line of a long fixture file, filler text for paging.";
export const LONG_NOTES = Array.from({ length: 400 }, (_, i) => `${String(i + 1).padStart(3, "0")} ${LONG_LINE}`).join("\n") + "\n";

type Served =
  | { kind: "html"; text: string; assets: string[] }
  | { kind: "markdown" | "text"; text: string }
  | { kind: "image"; mime: string; bytes: Uint8Array }
  | { kind: "binary"; size: number }
  | { error: string; code: DesktopErrorCode };

function base64Bytes(base64: string): Uint8Array {
  const binary = atob(base64);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

/** Canonical absolute path -> what the desktop would serve. */
const FILES: Record<string, Served> = {
  [`${MENTION_ROOT}/report.html`]: { kind: "html", text: REPORT_HTML, assets: ["img/chart.png"] },
  [`${MENTION_ROOT}/docs/findings.md`]: { kind: "markdown", text: FINDINGS_MD },
  [`${MENTION_ROOT}/docs/img/chart.png`]: { kind: "image", mime: "image/png", bytes: base64Bytes(PNG_BASE64) },
  [`${MENTION_ROOT}/img/chart.png`]: { kind: "image", mime: "image/png", bytes: base64Bytes(PNG_BASE64) },
  [`${MENTION_ROOT}/diagram.svg`]: { kind: "image", mime: "image/svg+xml", bytes: encoder.encode(SVG_TEXT) },
  [`${MENTION_ROOT}/src/example.ts`]: { kind: "text", text: 'export const answer: number = 42;\nexport function greet(name: string) {\n  return `Hello ${name}`;\n}\n' },
  "/Users/example/Documents/x/notes.py": { kind: "text", text: 'def main():\n    print("example notes")\n\n\nmain()\n' },
  [`${MENTION_ROOT}/long-notes.txt`]: { kind: "text", text: LONG_NOTES },
  [`${MENTION_ROOT}/bin/tool.bin`]: { kind: "binary", size: 48_213 },
  [`${MENTION_ROOT}/.env`]: { error: "This file is hidden", code: "path_denied" },
  [`${MENTION_ROOT}/big.log`]: { error: "This file is too large to preview", code: "too_large" },
  [`${MENTION_ROOT}/missing.txt`]: { error: "No such file", code: "not_found" },
};

/** Mentions as written in the messages, which of them points at which file. Anything else is not mentioned. */
const MENTIONS: Record<string, string> = {
  "report.html": `${MENTION_ROOT}/report.html`,
  "docs/findings.md": `${MENTION_ROOT}/docs/findings.md`,
  "img/chart.png": `${MENTION_ROOT}/img/chart.png`,
  "diagram.svg": `${MENTION_ROOT}/diagram.svg`,
  "src/example.ts": `${MENTION_ROOT}/src/example.ts`,
  "~/Documents/x/notes.py": "/Users/example/Documents/x/notes.py",
  "long-notes.txt": `${MENTION_ROOT}/long-notes.txt`,
  "bin/tool.bin": `${MENTION_ROOT}/bin/tool.bin`,
  ".env": `${MENTION_ROOT}/.env`,
  "big.log": `${MENTION_ROOT}/big.log`,
  "missing.txt": `${MENTION_ROOT}/missing.txt`,
  [`${MENTION_ROOT}/report.html`]: `${MENTION_ROOT}/report.html`,
};

/** The assistant text the mock conversations with mentions carry (every path above is in it, so it is realistic). */
export const MENTION_MESSAGE =
  "I wrote the report to `report.html` and the findings to `docs/findings.md` (chart: `img/chart.png`, diagram: [diagram](diagram.svg)).\n\n" +
  "The change is in `src/example.ts:2`, my notes are in `~/Documents/x/notes.py#L3` and `long-notes.txt`. " +
  "The build left `bin/tool.bin`, `big.log` and `missing.txt`, and I did not touch `.env`.";

export class MentionError extends Error {
  readonly code: DesktopErrorCode;
  constructor(message: string, code: DesktopErrorCode) {
    super(message);
    this.name = "MentionError";
    this.code = code;
  }
}

function base64Of(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

function lastName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

/** A safe relative asset path as the contract wants it, or null. */
function safeAsset(asset: string): string | null {
  if (/^[a-z][a-z0-9+.-]*:/i.test(asset) || asset.startsWith("/") || asset.startsWith("~") || asset.includes("\\") || asset.includes("\0")) return null;
  const parts = asset.split("/");
  if (parts.some((part) => part === ".." || part === "")) return null;
  return parts.filter((part) => part !== ".").join("/");
}

function serve(canonical: string, requested: Served, offset: number, source = false): MentionedFileResult {
  if ("error" in requested) throw new MentionError(requested.error, requested.code);
  // `source`: an html or markdown file answers as its raw text, like any text file (for a jump to a line).
  const served: Served = source && (requested.kind === "html" || requested.kind === "markdown") ? { kind: "text", text: requested.text } : requested;
  const name = lastName(canonical);
  if (served.kind === "binary") return { path: canonical, name, size: served.size, kind: "binary", encoding: "none", data: "", offset: 0, total: served.size };
  const bytes = served.kind === "image" ? served.bytes : encoder.encode(served.text);
  const end = Math.min(bytes.length, offset + MOCK_PAGE_BYTES);
  const page = bytes.subarray(offset, end);
  // The fixtures are ASCII, so a byte cut is also a character cut.
  const data = served.kind === "image" ? base64Of(page) : new TextDecoder().decode(page);
  return {
    path: canonical,
    name,
    size: bytes.length,
    kind: served.kind,
    ...(served.kind === "image" ? { mime: served.mime } : {}),
    encoding: served.kind === "image" ? "base64" : "utf8",
    data,
    offset,
    total: bytes.length,
    ...(end < bytes.length ? { nextOffset: end } : {}),
    ...(served.kind === "html" ? { assets: [...served.assets] } : {}),
    ...(canonical.startsWith(`${MENTION_ROOT}/`) ? { project: { projectId: "proj-alpha", path: canonical.slice(MENTION_ROOT.length + 1) } } : {}),
  };
}

/** The reply of `chat.mentionedFile`, or a `MentionError`. The mention check comes before anything else. */
export function mentionedFile(params: { path: string; asset?: string | undefined; offset?: number | undefined; source?: boolean | undefined }): MentionedFileResult {
  const raw = params.path.replace(/^\.\//, "");
  const canonical = MENTIONS[raw];
  if (!canonical) throw new MentionError("This file is not mentioned in this conversation", "forbidden");
  const served = FILES[canonical];
  if (!served) throw new MentionError("This file is not mentioned in this conversation", "forbidden");
  if (params.asset === undefined) return serve(canonical, served, params.offset ?? 0, params.source === true);
  const asset = safeAsset(params.asset);
  if (asset === null) throw new MentionError("This asset is not allowed", "forbidden");
  const folder = canonical.slice(0, canonical.lastIndexOf("/"));
  const target = FILES[`${folder}/${asset}`];
  if (!target) throw new MentionError("No such file", "not_found");
  if (!("kind" in target) || target.kind !== "image") throw new MentionError("This asset is not an image", "forbidden");
  return serve(`${folder}/${asset}`, target, params.offset ?? 0);
}
