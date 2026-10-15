import {
  Facet,
  Prec,
  StateEffect,
  StateField,
  type ChangeSet,
  type EditorState,
  type Extension,
  type Text,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  WidgetType,
  gutter,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import type { GitBlame, GitBlameHunk } from "../../../platform/tauri/fs";

export type BlameOpenCommit = (hunk: GitBlameHunk) => void;

export type EditorBlameOptions = {
  /** Show the current-line inline annotation. */
  inline: boolean;
  /** Show the blame gutter. */
  column: boolean;
  onOpenCommit?: BlameOpenCommit;
};

const NOT_COMMITTED_INLINE = "Not committed yet";
const NOT_COMMITTED_COLUMN = "Uncommitted";

const optionsFacet = Facet.define<EditorBlameOptions, EditorBlameOptions>({
  combine: (values) =>
    values[0] ?? { inline: false, column: false, onOpenCommit: undefined },
});

const setBlameEffect = StateEffect.define<GitBlame | null>();

/** Dispatch blame for the document as it is on disk. `null` (or a blame with
 * no hunks) says the file has none, e.g. it is untracked, binary or too
 * large: the annotations go away and the blame column collapses. Before the
 * first call the blame is merely not loaded yet, and the column keeps its width
 * so the code does not jump when blame arrives. */
export const setBlame = (view: EditorView, blame: GitBlame | null) => {
  view.dispatch({ effects: setBlameEffect.of(blame) });
};

/** The empty line CodeMirror shows after a final newline. Git has no such
 * line ("a\nb\n" is two lines to it, three to CodeMirror), so it is neither
 * committed nor new text and gets no annotation. */
const PAST_END = Symbol("blame-past-end");

/** The hunk of each document line (index = line number - 1); `null` for a
 * line that is not part of any hunk, e.g. one typed since the last save, and
 * `PAST_END` for the trailing empty line of a file that ends in a newline. */
type BlameLines = ReadonlyArray<GitBlameHunk | null | typeof PAST_END>;

function linesFromBlame(blame: GitBlame, doc: Text): BlameLines {
  const lines: Array<GitBlameHunk | null | typeof PAST_END> = new Array(
    doc.lines,
  ).fill(null);
  let blamed = 0;
  for (const hunk of blame.hunks) {
    const end = Math.min(doc.lines, hunk.startLine + hunk.lineCount - 1);
    for (let line = Math.max(1, hunk.startLine); line <= end; line++) {
      lines[line - 1] = hunk;
    }
    blamed = Math.max(blamed, end);
  }
  if (
    doc.lines > 1 &&
    blamed === doc.lines - 1 &&
    doc.line(doc.lines).length === 0
  ) {
    lines[doc.lines - 1] = PAST_END;
  }
  return lines;
}

/** Carry hunks across an edit. Blame describes the saved file, so a line keeps
 * its hunk while it is edited, a line that an edit adds has none, and removed
 * lines drop out instead of shifting authorship onto their neighbours.
 *
 * Each edit is read as one splice of whole lines: the first resulting line
 * continues the first old line only if text before the edit survives on it,
 * the last resulting line continues the last old line only if text after the
 * edit survives on it, and lines in between are new. Pressing Enter at the
 * start of a line therefore leaves the blank line unblamed and keeps the
 * hunk on the line that moved down. */
export function mapBlameLines(
  lines: BlameLines,
  changes: ChangeSet,
  oldDoc: Text,
  newDoc: Text,
): BlameLines {
  // Edits that touch the same old line are one splice, otherwise the second
  // would read a line the first already rewrote.
  const splices: Array<{
    fromA: number;
    toA: number;
    fromB: number;
    toB: number;
  }> = [];
  changes.iterChanges((fromA, toA, fromB, toB) => {
    const last = splices[splices.length - 1];
    if (last && oldDoc.lineAt(fromA).number <= oldDoc.lineAt(last.toA).number) {
      last.toA = toA;
      last.toB = toB;
    } else {
      splices.push({ fromA, toA, fromB, toB });
    }
  });

  // A line an edit touches is new text, so the trailing line stops being
  // "past the end" as soon as anything is typed on it.
  const carry = (line: number) => {
    const value = lines[line - 1] ?? null;
    return value === PAST_END ? null : value;
  };
  const out: Array<GitBlameHunk | null | typeof PAST_END> = [];
  let next = 1;
  for (const splice of splices) {
    const first = oldDoc.lineAt(splice.fromA);
    const last = oldDoc.lineAt(splice.toA);
    for (; next < first.number; next++) out.push(lines[next - 1] ?? null);
    const keepsPrefix = splice.fromA > first.from;
    const keepsSuffix =
      splice.toA < last.to ||
      // Enter on an empty line: the old line moves down, the new one is above.
      (splice.fromA === splice.toA && splice.fromA === first.from);
    const count =
      newDoc.lineAt(splice.toB).number - newDoc.lineAt(splice.fromB).number + 1;
    if (count === 1) {
      // With neither end surviving, the line is new text unless the edit only
      // removed text (what is left is the old last line) or replaced inside
      // one line. Replacing several lines with new text keeps no hunk.
      const owner = keepsPrefix
        ? first.number
        : keepsSuffix ||
            first.number === last.number ||
            splice.toB === splice.fromB
          ? last.number
          : null;
      out.push(owner === null ? null : carry(owner));
    } else {
      out.push(keepsPrefix ? carry(first.number) : null);
      for (let i = 2; i < count; i++) out.push(null);
      out.push(keepsSuffix ? carry(last.number) : null);
    }
    next = last.number + 1;
  }
  for (; next <= oldDoc.lines; next++) out.push(lines[next - 1] ?? null);
  return out;
}

/** `null` until blame is first set, `"none"` once the file is known to have
 * none, otherwise the hunk of each line. */
type BlameState = BlameLines | "none" | null;

const blameField = StateField.define<BlameState>({
  create: () => null,
  update(value, tr) {
    if (value && value !== "none" && tr.docChanged) {
      value = mapBlameLines(value, tr.changes, tr.startState.doc, tr.newDoc);
    }
    // A fresh blame is built against the new document and replaces the map.
    for (const effect of tr.effects) {
      if (effect.is(setBlameEffect)) {
        value =
          effect.value && effect.value.hunks.length > 0
            ? linesFromBlame(effect.value, tr.newDoc)
            : "none";
      }
    }
    return value;
  },
});

/** The hunk of a document line, or `null` when it has none (not loaded, typed
 * since the last save, or outside the blamed file). */
export function blameForLine(
  state: EditorState,
  lineNumber: number,
): GitBlameHunk | null {
  const value = lineValue(state, lineNumber);
  return value === PAST_END ? null : value;
}

function lineValue(state: EditorState, lineNumber: number) {
  const lines = state.field(blameField, false);
  return Array.isArray(lines) ? (lines[lineNumber - 1] ?? null) : null;
}

/** True for the empty line after a final newline, which carries no blame. */
function isPastEnd(state: EditorState, lineNumber: number): boolean {
  return lineValue(state, lineNumber) === PAST_END;
}

function hasBlame(state: EditorState): boolean {
  return Array.isArray(state.field(blameField, false));
}

/** Hunks that count as the same block in the gutter; every uncommitted or
 * unblamed line shares one block. */
function blockOf(state: EditorState, lineNumber: number) {
  const hunk = blameForLine(state, lineNumber);
  return hunk && !hunk.uncommitted ? hunk : null;
}

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

export function formatBlameAge(authorTimeSec: number, nowMs: number): string {
  const seconds = Math.floor(nowMs / 1000 - authorTimeSec);
  const unit = (n: number, name: string) =>
    `${n} ${name}${n === 1 ? "" : "s"} ago`;
  if (seconds < MINUTE) return "just now";
  if (seconds < HOUR) return unit(Math.floor(seconds / MINUTE), "minute");
  if (seconds < DAY) return unit(Math.floor(seconds / HOUR), "hour");
  const days = Math.floor(seconds / DAY);
  if (days < 30) return unit(days, "day");
  if (days < 365) return unit(Math.max(1, Math.floor(days / 30.4375)), "month");
  return unit(Math.floor(days / 365), "year");
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** `YYYY-MM-DD` in local time. */
function shortDate(authorTimeSec: number): string {
  const date = new Date(authorTimeSec * 1000);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** `YYYY-MM-DD HH:MM` in local time. */
function fullDate(authorTimeSec: number): string {
  const date = new Date(authorTimeSec * 1000);
  return `${shortDate(authorTimeSec)} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function inlineText(hunk: GitBlameHunk | null, nowMs: number): string {
  if (!hunk || hunk.uncommitted) return NOT_COMMITTED_INLINE;
  return `${hunk.author}, ${formatBlameAge(hunk.authorTime, nowMs)} • ${hunk.summary}`;
}

/** A stable hue per commit so neighbouring blocks read as separate. */
function hunkHue(sha: string): number {
  let hash = 0;
  for (let i = 0; i < sha.length; i++) {
    hash = (hash * 31 + sha.charCodeAt(i)) >>> 0;
  }
  return hash % 360;
}

class BlameHover {
  private tip: HTMLElement | null = null;
  private readonly onScroll = () => this.hide();

  constructor(readonly view: EditorView) {
    // Scroll does not bubble; capture also sees ancestors of the editor, which
    // would otherwise leave the fixed tooltip floating.
    window.addEventListener("scroll", this.onScroll, {
      capture: true,
      passive: true,
    });
  }

  update(update: ViewUpdate) {
    // The browser fires no mouseleave when the element under the pointer is
    // replaced, so anything that can move or rebuild the anchor hides the tip.
    if (
      update.docChanged ||
      update.selectionSet ||
      update.focusChanged ||
      update.viewportChanged ||
      update.geometryChanged ||
      update.startState.field(blameField, false) !==
        update.state.field(blameField, false)
    ) {
      this.hide();
    }
  }

  destroy() {
    this.hide();
    window.removeEventListener("scroll", this.onScroll, { capture: true });
  }

  show(anchor: HTMLElement, hunk: GitBlameHunk | null) {
    this.hide();
    const tip = document.createElement("div");
    tip.className = "cm-tooltip cm-blameTooltip";
    const row = (className: string, text: string) => {
      const el = document.createElement("div");
      el.className = className;
      el.textContent = text;
      tip.append(el);
    };
    if (!hunk || hunk.uncommitted) {
      row("cm-blameTooltipSummary", NOT_COMMITTED_INLINE);
    } else {
      row("cm-blameTooltipMeta", `${hunk.shortSha}  ${hunk.author}`);
      row("cm-blameTooltipMeta", fullDate(hunk.authorTime));
      row("cm-blameTooltipSummary", hunk.summary);
      if (this.view.state.facet(optionsFacet).onOpenCommit) {
        row("cm-blameTooltipHint", "Click to open commit");
      }
    }
    this.view.dom.append(tip);
    const rect = anchor.getBoundingClientRect();
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const left = Math.max(
      8,
      Math.min(rect.left, window.innerWidth - width - 8),
    );
    const below = rect.bottom + 4;
    const top =
      below + height > window.innerHeight - 8
        ? Math.max(8, rect.top - height - 4)
        : below;
    tip.style.left = `${left}px`;
    tip.style.top = `${top}px`;
    this.tip = tip;
  }

  hide() {
    this.tip?.remove();
    this.tip = null;
  }
}

const blameHover = ViewPlugin.fromClass(BlameHover);

/** Hover and click behaviour shared by the inline annotation and gutter cells. */
function wireBlameElement(
  view: EditorView,
  el: HTMLElement,
  hunk: GitBlameHunk | null,
) {
  el.addEventListener("mouseenter", () => {
    view.plugin(blameHover)?.show(el, hunk);
  });
  el.addEventListener("mouseleave", () => {
    view.plugin(blameHover)?.hide();
  });
  if (hunk && !hunk.uncommitted) {
    el.addEventListener("click", (event) => {
      const open = view.state.facet(optionsFacet).onOpenCommit;
      if (!open) return;
      event.preventDefault();
      view.plugin(blameHover)?.hide();
      open(hunk);
    });
  }
}

function isClickable(state: EditorState, hunk: GitBlameHunk | null): boolean {
  return (
    !!hunk && !hunk.uncommitted && !!state.facet(optionsFacet).onOpenCommit
  );
}

class InlineBlameWidget extends WidgetType {
  constructor(
    readonly hunk: GitBlameHunk | null,
    readonly text: string,
    readonly clickable: boolean,
  ) {
    super();
  }

  eq(other: InlineBlameWidget) {
    return (
      other.hunk === this.hunk &&
      other.text === this.text &&
      other.clickable === this.clickable
    );
  }

  toDOM(view: EditorView) {
    const el = document.createElement("span");
    el.className = "cm-blameInline";
    if (this.clickable) el.classList.add("cm-blameClickable");
    el.setAttribute("aria-hidden", "true");
    const text = document.createElement("span");
    text.textContent = this.text;
    el.append(text);
    wireBlameElement(view, el, this.hunk);
    // ignoreEvent keeps CodeMirror out of it, but the browser would still move
    // the caret or start a drag selection from a click on the annotation.
    if (this.clickable) {
      el.addEventListener("mousedown", (event) => event.preventDefault());
    }
    return el;
  }

  ignoreEvent() {
    return true;
  }
}

function inlineDecorations(view: EditorView): DecorationSet {
  const { state } = view;
  const selection = state.selection.main;
  if (
    !state.facet(optionsFacet).inline ||
    !hasBlame(state) ||
    !selection.empty ||
    !view.hasFocus
  ) {
    return Decoration.none;
  }
  const line = state.doc.lineAt(selection.head);
  if (isPastEnd(state, line.number)) return Decoration.none;
  const hunk = blameForLine(state, line.number);
  const widget = new InlineBlameWidget(
    hunk,
    inlineText(hunk, Date.now()),
    isClickable(state, hunk),
  );
  return Decoration.set([
    Decoration.widget({ widget, side: 1 }).range(line.to),
  ]);
}

const inlineBlame = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = inlineDecorations(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.focusChanged ||
        update.startState.field(blameField, false) !==
          update.state.field(blameField, false) ||
        update.startState.facet(optionsFacet) !==
          update.state.facet(optionsFacet)
      ) {
        this.decorations = inlineDecorations(update.view);
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

class BlameCell extends GutterMarker {
  constructor(
    /** `null` for an uncommitted or unblamed line. */
    readonly hunk: GitBlameHunk | null,
    readonly first: boolean,
    readonly clickable: boolean,
  ) {
    super();
  }

  eq(other: BlameCell) {
    return (
      other.hunk === this.hunk &&
      other.first === this.first &&
      other.clickable === this.clickable
    );
  }

  toDOM(view: EditorView) {
    const cell = document.createElement("div");
    cell.className = "cm-blameCell";
    if (this.clickable) cell.classList.add("cm-blameClickable");
    cell.style.setProperty(
      "--cm-blame-tint",
      this.hunk
        ? `hsl(${hunkHue(this.hunk.sha)} 55% 55% / 0.55)`
        : "color-mix(in srgb, var(--color-content) 18%, transparent)",
    );
    if (this.first) {
      const author = document.createElement("span");
      author.className = "cm-blameAuthor";
      author.textContent = this.hunk ? this.hunk.author : NOT_COMMITTED_COLUMN;
      cell.append(author);
      if (this.hunk) {
        const date = document.createElement("span");
        date.className = "cm-blameDate";
        date.textContent = shortDate(this.hunk.authorTime);
        cell.append(date);
      }
    }
    wireBlameElement(view, cell, this.hunk);
    return cell;
  }
}

/** Reserves the column's width before blame arrives, so the code does not jump. */
const spacerCell = new BlameCell(null, false, false);

const blameGutter = gutter({
  class: "cm-blameGutter",
  lineMarker(view, line) {
    const { state } = view;
    if (!hasBlame(state)) return null;
    const number = state.doc.lineAt(line.from).number;
    if (isPastEnd(state, number)) return null;
    const hunk = blockOf(state, number);
    const first = number === 1 || blockOf(state, number - 1) !== hunk;
    return new BlameCell(hunk, first, isClickable(state, hunk));
  },
  lineMarkerChange: (update) =>
    update.docChanged ||
    update.startState.field(blameField, false) !==
      update.state.field(blameField, false) ||
    update.startState.facet(optionsFacet) !== update.state.facet(optionsFacet),
  initialSpacer: () => spacerCell,
});

/** Collapses the column of a file with no blame. */
const noBlameClass = EditorView.editorAttributes.compute(
  [blameField],
  (state) => ({
    class: state.field(blameField, false) === "none" ? "cm-blameNone" : "",
  }),
);

const mutedBlame = "color-mix(in srgb, var(--color-content) 40%, transparent)";

const blameTheme = EditorView.baseTheme({
  ".cm-blameInline": {
    marginLeft: "4ch",
    color: mutedBlame,
    userSelect: "none",
    pointerEvents: "none",
    whiteSpace: "pre",
  },
  ".cm-blameInline > span": {
    pointerEvents: "auto",
  },
  ".cm-blameInline.cm-blameClickable > span, .cm-blameCell.cm-blameClickable": {
    cursor: "pointer",
  },
  // CodeMirror's own .cm-gutter rule is display: flex !important.
  "&.cm-blameNone .cm-blameGutter": {
    display: "none !important",
  },
  ".cm-blameGutter .cm-gutterElement": {
    padding: "0",
  },
  ".cm-blameCell": {
    display: "flex",
    alignItems: "baseline",
    gap: "1ch",
    boxSizing: "border-box",
    width: "22ch",
    height: "100%",
    padding: "0 6px 0 6px",
    borderLeft: "2px solid var(--cm-blame-tint)",
    color: mutedBlame,
    fontSize: "11px",
    whiteSpace: "pre",
    userSelect: "none",
  },
  ".cm-blameAuthor": {
    flex: "1 1 auto",
    minWidth: "0",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
  ".cm-blameDate": {
    flex: "none",
  },
  ".cm-blameTooltip": {
    position: "fixed",
    zIndex: "500",
    padding: "6px 10px",
    maxWidth: "420px",
    fontFamily: "var(--font-mono)",
    fontSize: "12px",
    pointerEvents: "none",
  },
  ".cm-blameTooltipMeta": {
    color: mutedBlame,
  },
  ".cm-blameTooltipSummary": {
    margin: "4px 0",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  },
  ".cm-blameTooltipHint": {
    color: mutedBlame,
    fontSize: "11px",
  },
});

export function editorBlame(options: EditorBlameOptions): Extension {
  if (!options.inline && !options.column) return [];
  return [
    optionsFacet.of(options),
    blameField,
    blameHover,
    options.inline ? inlineBlame : [],
    // Highest precedence puts the column before the line numbers, at the far
    // left, where blame reads as a margin note on the whole file.
    options.column ? [Prec.high(blameGutter), noBlameClass] : [],
    blameTheme,
  ];
}
