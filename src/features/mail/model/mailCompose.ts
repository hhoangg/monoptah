/**
 * TipTap document -> the three things an email needs (spec 7.5).
 *
 * Pure data transform: TipTap JSON is plain data, so no editor, React or extra
 * dependency is needed. This is the write path; MailMessageBody is the read
 * path and the two deliberately share no code.
 */

export type TipTapMark = { type: string; attrs?: Record<string, unknown> };
export type TipTapNode = {
  type: string;
  attrs?: Record<string, unknown>;
  content?: TipTapNode[];
  marks?: TipTapMark[];
  text?: string;
};

/**
 * The part of an outbox row this module reads. Recipients (including Bcc) are
 * not read: Bcc belongs in SES's Destination only, so it must never reach a
 * body even by accident.
 */
export type ComposeInput = {
  bodyJson: TipTapNode;
  /** Salts generated content ids; see contentIdFor. */
  draftId: string;
  /** Already sanitised by ammonia (Task 3); appended as-is, never re-processed. */
  quotedHtml?: string | null;
};

export type InlineImage = {
  /** The pending attachment id carried by the Image node (`data-mono-att`). */
  attachmentId: string;
  /** Matches `mail_attachment.content_id` exactly (stored without `<>`). */
  contentId: string;
  alt: string | null;
};

export type ComposeOutput = {
  html: string;
  text: string;
  inlineImages: InlineImage[];
};

const FONT = "-apple-system,'Segoe UI',Helvetica,Arial,sans-serif";
const MONO = "ui-monospace,Menlo,Consolas,monospace";
const RULE = "#d0d0d0";
const TINT = "#f4f4f4";

// Email clients strip <style>, so every element carries its own style.
const STYLE: Record<string, string> = {
  p: "margin:0 0 1em",
  h1: "font-size:1.6em;font-weight:700;margin:0 0 .6em",
  h2: "font-size:1.35em;font-weight:700;margin:0 0 .6em",
  h3: "font-size:1.15em;font-weight:700;margin:0 0 .6em",
  ul: "margin:0 0 1em;padding-left:24px",
  ol: "margin:0 0 1em;padding-left:24px",
  li: "margin:0 0 .25em",
  blockquote: `margin:0 0 1em;padding-left:12px;border-left:2px solid ${RULE}`,
  pre: `margin:0 0 1em;padding:8px 12px;background:${TINT};border-radius:4px;white-space:pre-wrap`,
  codeBlock: `font-family:${MONO}`,
  code: `font-family:${MONO};background:${TINT};padding:1px 4px;border-radius:3px`,
  hr: `border:0;border-top:1px solid ${RULE};margin:1em 0`,
  a: "color:#1a5fb4;text-decoration:underline",
  strong: "font-weight:700",
  em: "font-style:italic",
  u: "text-decoration:underline",
  s: "text-decoration:line-through",
  img: "max-width:100%;height:auto",
  div: "margin:0 0 1em",
};

const MARK_TAG: Record<string, string> = {
  bold: "strong",
  italic: "em",
  underline: "u",
  strike: "s",
  code: "code",
};

const SAFE_HREF = /^(https?:|mailto:)/i;

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const open = (tag: string, extra = "") =>
  `<${tag}${extra} style="${escapeHtml(STYLE[tag])}">`;

const escapeId = (id: string) =>
  // Uppercase is escaped, not folded: the reader lowercases cids, so folding
  // would make "A" and "a" the same cid. Every escape is "_" plus exactly four
  // hex digits (one UTF-16 unit), and "_" itself is escaped, so decoding is
  // unambiguous and this is injective; "." (the joiner below) is never emitted.
  id.replace(
    /[^a-z0-9-]/g,
    (c) => `_${c.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );

/**
 * The draft id salts the cid so a stale `cid:` inside the appended quoted
 * original (from an earlier Monoptah message) can never match a new image.
 * A salt rather than randomness: reopening a draft must give identical HTML.
 */
export function contentIdFor(draftId: string, attachmentId: string): string {
  return `mono-${escapeId(draftId)}.${escapeId(attachmentId)}@monoptah.local`;
}

// NUL/C0 controls and lone surrogates corrupt MIME or become U+FFFD on UTF-8
// encoding; tab and newline are real content.
const BAD_CHARS =
  // eslint-disable-next-line no-control-regex
  /[\ud800-\udbff][\udc00-\udfff]|[\u0000-\u0008\u000b-\u001f\ud800-\udfff]/g;

function clean(n: TipTapNode): TipTapNode {
  const str = (v: unknown) =>
    typeof v === "string"
      ? v.replace(BAD_CHARS, (m) => (m.length === 2 ? m : ""))
      : v;
  return {
    ...n,
    ...(n.text !== undefined ? { text: str(n.text) as string } : {}),
    ...(n.attrs
      ? {
          attrs: Object.fromEntries(
            Object.entries(n.attrs).map(([k, v]) => [k, str(v)]),
          ),
        }
      : {}),
    ...(n.marks
      ? {
          marks: n.marks.map((m) => ({
            ...m,
            ...(m.attrs
              ? {
                  attrs: Object.fromEntries(
                    Object.entries(m.attrs).map(([k, v]) => [k, str(v)]),
                  ),
                }
              : {}),
          })),
        }
      : {}),
    ...(n.content ? { content: n.content.map(clean) } : {}),
  };
}

const isInline = (n: TipTapNode) => n.type === "text" || n.type === "hardBreak";

function linkHref(n: TipTapNode): string | null {
  const href = n.marks?.find((m) => m.type === "link")?.attrs?.href;
  return typeof href === "string" && SAFE_HREF.test(href.trim())
    ? href.trim()
    : null;
}

/** Adjacent nodes under one link become one run, so its URL is appended once. */
function runs(
  nodes: TipTapNode[],
): { href: string | null; nodes: TipTapNode[] }[] {
  const out: { href: string | null; nodes: TipTapNode[] }[] = [];
  for (const n of nodes) {
    const href = linkHref(n);
    const last = out[out.length - 1];
    if (last && href !== null && last.href === href) last.nodes.push(n);
    else out.push({ href, nodes: [n] });
  }
  return out;
}

function attId(n: TipTapNode): string | null {
  const v = n.attrs?.["data-mono-att"];
  const id = typeof v === "string" || typeof v === "number" ? String(v) : "";
  // An empty id would yield a malformed cid; treat it as no attachment.
  return id === "" ? null : id;
}

const altOf = (n: TipTapNode) =>
  typeof n.attrs?.alt === "string" && n.attrs.alt ? n.attrs.alt : null;

// ---------- HTML ----------

type Ctx = { draftId: string; images: Map<string, InlineImage> };

function imageHtml(n: TipTapNode, ctx: Ctx): string {
  const id = attId(n);
  const alt = altOf(n);
  if (id === null) {
    // No pending attachment means no bytes to send; show it rather than drop it.
    return escapeHtml(`[image${alt ? `: ${alt}` : ""}]`);
  }
  const contentId = contentIdFor(ctx.draftId, id);
  if (!ctx.images.has(id))
    ctx.images.set(id, { attachmentId: id, contentId, alt });
  return `<img src="cid:${contentId}" alt="${escapeHtml(alt ?? "")}" style="${escapeHtml(STYLE.img)}">`;
}

function textHtml(n: TipTapNode): string {
  let out = escapeHtml(n.text ?? "");
  for (const m of n.marks ?? []) {
    const tag = MARK_TAG[m.type];
    if (tag) out = `${open(tag)}${out}</${tag}>`;
  }
  return out;
}

function inlineHtml(nodes: TipTapNode[], ctx: Ctx): string {
  return runs(nodes)
    .map(({ href, nodes: ns }) => {
      const inner = ns
        .map((n) =>
          n.type === "hardBreak"
            ? "<br>"
            : n.type === "image"
              ? imageHtml(n, ctx)
              : textHtml(n),
        )
        .join("");
      return href
        ? `${open("a", ` href="${escapeHtml(href)}"`)}${inner}</a>`
        : inner;
    })
    .join("");
}

function childrenHtml(nodes: TipTapNode[] = [], ctx: Ctx): string {
  return nodes.map((n) => blockHtml(n, ctx)).join("");
}

/** A lone paragraph is inlined so its 1em margin does not defeat the li gap. */
function itemHtml(kids: TipTapNode[] = [], ctx: Ctx): string {
  return kids.length === 1 &&
    kids[0].type === "paragraph" &&
    kids[0].content?.length
    ? inlineHtml(kids[0].content, ctx)
    : childrenHtml(kids, ctx);
}

function listHtml(tag: "ul" | "ol", n: TipTapNode, ctx: Ctx): string {
  const start = tag === "ol" ? Number(n.attrs?.start ?? 1) : 1;
  const attr = tag === "ol" && start !== 1 ? ` start="${start}"` : "";
  const items = (n.content ?? [])
    .map((li) =>
      li.type === "listItem"
        ? `${open("li")}${itemHtml(li.content, ctx)}</li>`
        : `${open("li")}${blockHtml(li, ctx)}</li>`,
    )
    .join("");
  return `${open(tag, attr)}${items}</${tag}>`;
}

function blockHtml(n: TipTapNode, ctx: Ctx): string {
  const kids = n.content ?? [];
  switch (n.type) {
    case "paragraph":
      return `${open("p")}${kids.length ? inlineHtml(kids, ctx) : "<br>"}</p>`;
    case "heading": {
      const level = Math.min(3, Math.max(1, Number(n.attrs?.level ?? 1)));
      const tag = `h${level}`;
      return `${open(tag)}${inlineHtml(kids, ctx)}</${tag}>`;
    }
    case "bulletList":
      return listHtml("ul", n, ctx);
    case "orderedList":
      return listHtml("ol", n, ctx);
    case "blockquote":
      return `${open("blockquote")}${childrenHtml(kids, ctx)}</blockquote>`;
    case "codeBlock": {
      const code = escapeHtml(kids.map((k) => k.text ?? "").join(""));
      return `${open("pre")}<code style="${escapeHtml(STYLE.codeBlock)}">${code}</code></pre>`;
    }
    case "horizontalRule":
      return `<hr style="${escapeHtml(STYLE.hr)}">`;
    case "image":
      return `${open("p")}${imageHtml(n, ctx)}</p>`;
    default:
      // Unknown node: keep whatever text it holds in a visible block, because
      // dropping it would send a message missing something the user wrote.
      if (!kids.length) return `${open("p")}${escapeHtml(`[${n.type}]`)}</p>`;
      return `${open("div")}${kids.some(isInline) ? inlineHtml(kids, ctx) : childrenHtml(kids, ctx)}</div>`;
  }
}

// ---------- plain text ----------

function inlineText(nodes: TipTapNode[]): string {
  return runs(nodes)
    .map(({ href, nodes: ns }) => {
      const text = ns
        .map((n) =>
          n.type === "hardBreak"
            ? "\n"
            : n.type === "image"
              ? `[image${altOf(n) ? `: ${altOf(n)}` : ""}]`
              : (n.text ?? ""),
        )
        .join("");
      if (!href) return text;
      const bare = href.replace(/^mailto:/i, "");
      return text.trim() === href || text.trim() === bare
        ? text
        : `${text} (${href})`;
    })
    .join("");
}

// Blank blocks are dropped here, while the HTML keeps an interior empty
// paragraph as <p><br></p>: in plain text a run of blank paragraphs reads as
// noise, so deliberate spacing collapses to one blank line.
const blocksText = (nodes: TipTapNode[] = []) =>
  nodes.map(blockText).filter((s) => s.trim() !== "");

/**
 * Nested lists: each item's continuation lines (including a child list) are
 * indented by the width of its own marker, "- " = 2, "1. " = 3, so the child
 * lines up under the item text. Each list picks its own marker, so an ordered
 * list inside a bullet list numbers from 1 and vice versa.
 */
function listText(n: TipTapNode, ordered: boolean): string {
  const start = Number(n.attrs?.start ?? 1);
  return (n.content ?? [])
    .map((li, i) => {
      const marker = ordered ? `${start + i}. ` : "- ";
      const body = blocksText(li.type === "listItem" ? li.content : [li]).join(
        "\n",
      );
      const pad = " ".repeat(marker.length);
      return body
        .split("\n")
        .map((line, j) => (j === 0 ? marker + line : line ? pad + line : line))
        .join("\n");
    })
    .join("\n");
}

function blockText(n: TipTapNode): string {
  const kids = n.content ?? [];
  switch (n.type) {
    case "paragraph":
    case "heading":
      return inlineText(kids);
    case "bulletList":
      return listText(n, false);
    case "orderedList":
      return listText(n, true);
    case "blockquote":
      return blocksText(kids)
        .join("\n\n")
        .split("\n")
        .map((l) => (l ? `> ${l}` : ">"))
        .join("\n");
    case "codeBlock":
      return kids.map((k) => k.text ?? "").join("");
    case "horizontalRule":
      return "---";
    case "image":
      return inlineText([n]);
    default:
      if (!kids.length) return `[${n.type}]`;
      return kids.some(isInline)
        ? inlineText(kids)
        : blocksText(kids).join("\n\n");
  }
}

// ---------- entry point ----------

const isEmptyParagraph = (n: TipTapNode) =>
  n.type === "paragraph" && !n.content?.length;

export function serializeCompose(input: ComposeInput): ComposeOutput {
  const ctx: Ctx = { draftId: input.draftId, images: new Map() };
  const top = [...(clean(input.bodyJson).content ?? [])];
  // TipTap keeps a trailing empty paragraph so the caret can leave a block; it
  // is editor furniture, not content the recipient should get as dead space.
  while (top.length && isEmptyParagraph(top[top.length - 1])) top.pop();

  const body = childrenHtml(top, ctx);
  const html =
    `<!DOCTYPE html><html><body style="font-family:${escapeHtml(FONT)};font-size:14px;line-height:1.5">` +
    body +
    (input.quotedHtml ?? "") +
    "</body></html>";

  return {
    html,
    text: blocksText(top).join("\n\n"),
    inlineImages: [...ctx.images.values()],
  };
}
