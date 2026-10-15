// @vitest-environment happy-dom
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GitBlame, GitBlameHunk } from "../../../platform/tauri/fs";
import {
  blameForLine,
  editorBlame,
  formatBlameAge,
  setBlame,
  type EditorBlameOptions,
} from "./editorBlame";

let view: EditorView | null = null;

afterEach(() => {
  view?.destroy();
  view = null;
  vi.useRealTimers();
});

const NOW = Date.UTC(2026, 5, 15, 12, 0, 0);
const SEC = NOW / 1000;

function hunk(
  sha: string,
  startLine: number,
  lineCount: number,
  extra: Partial<GitBlameHunk> = {},
): GitBlameHunk {
  return {
    sha: sha.padEnd(40, "0"),
    shortSha: sha.slice(0, 7),
    author: `author-${sha}`,
    authorTime: SEC - 2 * 86400,
    summary: `summary ${sha}`,
    uncommitted: false,
    startLine,
    lineCount,
    ...extra,
  };
}

const A = hunk("aaa1111", 1, 2);
const B = hunk("bbb2222", 3, 3);
const U = hunk("0", 6, 1, {
  author: "Not Committed Yet",
  summary: "Version of x from x",
  uncommitted: true,
});
const BLAME: GitBlame = { hunks: [A, B, U] };
const DOC = "one\ntwo\nthree\nfour\nfive\nsix";

function mount(
  options: Partial<EditorBlameOptions> = {},
  doc = DOC,
  blame: GitBlame | null = BLAME,
) {
  const parent = document.createElement("div");
  document.body.append(parent);
  view = new EditorView({
    state: EditorState.create({
      doc,
      extensions: [
        lineNumbers(),
        editorBlame({ inline: true, column: true, ...options }),
      ],
    }),
    parent,
  });
  if (blame) setBlame(view, blame);
  return view;
}

function focus(v: EditorView) {
  Object.defineProperty(v, "hasFocus", { get: () => true, configurable: true });
}

function cursorAt(v: EditorView, line: number, col = 0) {
  v.dispatch({
    selection: EditorSelection.cursor(v.state.doc.line(line).from + col),
  });
}

function owners(v: EditorView) {
  return Array.from({ length: v.state.doc.lines }, (_, i) =>
    blameForLine(v.state, i + 1),
  );
}

describe("formatBlameAge", () => {
  const ago = (seconds: number) => formatBlameAge(SEC - seconds, NOW);

  it("says just now under a minute, including clock skew", () => {
    expect(ago(0)).toBe("just now");
    expect(ago(59)).toBe("just now");
    expect(ago(-300)).toBe("just now");
  });

  it("switches units at the boundaries", () => {
    expect(ago(60)).toBe("1 minute ago");
    expect(ago(5 * 60)).toBe("5 minutes ago");
    expect(ago(3599)).toBe("59 minutes ago");
    expect(ago(3600)).toBe("1 hour ago");
    expect(ago(3 * 3600)).toBe("3 hours ago");
    expect(ago(86399)).toBe("23 hours ago");
    expect(ago(86400)).toBe("1 day ago");
    expect(ago(2 * 86400)).toBe("2 days ago");
    expect(ago(29 * 86400)).toBe("29 days ago");
    expect(ago(30 * 86400)).toBe("1 month ago");
    expect(ago(120 * 86400)).toBe("3 months ago");
    expect(ago(364 * 86400)).toBe("11 months ago");
    expect(ago(365 * 86400)).toBe("1 year ago");
    expect(ago(800 * 86400)).toBe("2 years ago");
  });
});

describe("blame line mapping", () => {
  it("assigns each line its hunk after setBlame", () => {
    mount();
    expect(owners(view!)).toEqual([A, A, B, B, B, U]);
  });

  it("clears with null and ignores lines past the end of the document", () => {
    mount({}, "one\ntwo");
    expect(owners(view!)).toEqual([A, A]);
    setBlame(view!, null);
    expect(owners(view!)).toEqual([null, null]);
  });

  it("gives an inserted line no hunk and keeps the lines after it", () => {
    mount();
    const v = view!;
    const end = v.state.doc.line(3).to;
    v.dispatch({ changes: { from: end, insert: "\nnew" } });
    expect(owners(v)).toEqual([A, A, B, null, B, B, U]);
  });

  it("keeps the hunk on the line that moves down when Enter splits at line start", () => {
    mount();
    const v = view!;
    v.dispatch({ changes: { from: v.state.doc.line(3).from, insert: "\n" } });
    expect(owners(v)).toEqual([A, A, null, B, B, B, U]);
  });

  it("keeps the hunk on the first half when Enter splits mid-line", () => {
    mount();
    const v = view!;
    v.dispatch({
      changes: { from: v.state.doc.line(1).from + 1, insert: "\n" },
    });
    expect(owners(v)).toEqual([A, A, A, B, B, B, U]);
  });

  it("keeps the hunk while a line is edited in place", () => {
    mount();
    const v = view!;
    v.dispatch({ changes: { from: v.state.doc.line(4).to, insert: "!" } });
    expect(owners(v)).toEqual([A, A, B, B, B, U]);
  });

  it("shrinks a hunk when its lines are deleted", () => {
    mount();
    const v = view!;
    const line = v.state.doc.line(4);
    v.dispatch({ changes: { from: line.from, to: line.to + 1 } });
    expect(owners(v)).toEqual([A, A, B, B, U]);
  });

  it("drops lines across a hunk boundary without shifting authorship", () => {
    mount();
    const v = view!;
    // Delete lines 2 and 3 entirely.
    v.dispatch({
      changes: { from: v.state.doc.line(2).from, to: v.state.doc.line(4).from },
    });
    expect(owners(v)).toEqual([A, B, B, U]);
  });

  it("keeps the first hunk when a deletion joins two lines mid-way", () => {
    mount();
    const v = view!;
    v.dispatch({
      changes: {
        from: v.state.doc.line(2).from + 1,
        to: v.state.doc.line(4).from + 1,
      },
    });
    expect(owners(v)).toEqual([A, A, B, U]);
  });

  it("handles several edits on one line as a single splice", () => {
    mount();
    const v = view!;
    const line = v.state.doc.line(3);
    v.dispatch({
      changes: [
        { from: line.from, insert: "[" },
        { from: line.to, insert: "]" },
      ],
    });
    expect(owners(v)).toEqual([A, A, B, B, B, U]);
  });

  it("drops the last line when the final newline is deleted with it", () => {
    mount();
    const v = view!;
    v.dispatch({
      changes: { from: v.state.doc.line(5).to, to: v.state.doc.length },
    });
    expect(owners(v)).toEqual([A, A, B, B, B]);
  });

  it("gives no hunk to text typed over several lines", () => {
    mount();
    const v = view!;
    // Select lines 1-5 fully and type "z".
    v.dispatch({
      changes: { from: 0, to: v.state.doc.line(5).to, insert: "z" },
    });
    expect(v.state.doc.toString()).toBe("z\nsix");
    expect(owners(v)).toEqual([null, U]);
  });

  it("keeps the remaining line's hunk when a multi-line deletion removes text only", () => {
    mount();
    const v = view!;
    // Delete lines 1-5 including the last newline: "six" is what is left.
    v.dispatch({
      changes: { from: 0, to: v.state.doc.line(6).from },
    });
    expect(owners(v)).toEqual([U]);
  });

  it("maps a bulk replace to the new line count with no stale hunks", () => {
    mount();
    const v = view!;
    v.dispatch({
      changes: { from: 0, to: v.state.doc.length, insert: "x\ny" },
    });
    expect(owners(v)).toEqual([null, null]);
  });
});

describe("inline blame", () => {
  const inlineText = (v: EditorView) =>
    [...v.dom.querySelectorAll(".cm-blameInline")].map((el) => el.textContent);

  function mountFocused(options: Partial<EditorBlameOptions> = {}) {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(NOW);
    const v = mount({ column: false, ...options });
    focus(v);
    return v;
  }

  it("annotates only the cursor line, in author, age, summary form", () => {
    const v = mountFocused();
    cursorAt(v, 4);
    expect(inlineText(v)).toEqual([
      "author-bbb2222, 2 days ago • summary bbb2222",
    ]);
    cursorAt(v, 1);
    expect(inlineText(v)).toEqual([
      "author-aaa1111, 2 days ago • summary aaa1111",
    ]);
  });

  it("sits at the end of its line", () => {
    const v = mountFocused();
    cursorAt(v, 2);
    const line = v.dom.querySelectorAll(".cm-line")[1];
    const widget = line.querySelector(".cm-blameInline")!;
    expect(widget.parentElement).toBe(line);
    expect(line.textContent).toBe(`two${widget.textContent}`);
  });

  it("says Not committed yet for uncommitted and unblamed lines, ignoring git's placeholders", () => {
    const v = mountFocused();
    cursorAt(v, 6);
    expect(inlineText(v)).toEqual(["Not committed yet"]);
    v.dispatch({ changes: { from: v.state.doc.line(2).to, insert: "\nnew" } });
    cursorAt(v, 3);
    expect(inlineText(v)).toEqual(["Not committed yet"]);
  });

  it("is hidden while a range is selected", () => {
    const v = mountFocused();
    cursorAt(v, 4);
    expect(inlineText(v)).toHaveLength(1);
    v.dispatch({ selection: EditorSelection.range(0, 5) });
    expect(inlineText(v)).toEqual([]);
  });

  it("is hidden without editor focus", () => {
    const v = mountFocused();
    Object.defineProperty(v, "hasFocus", {
      get: () => false,
      configurable: true,
    });
    cursorAt(v, 4);
    expect(inlineText(v)).toEqual([]);
  });

  it("is hidden before blame has loaded", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const v = mount({ column: false }, DOC, null);
    focus(v);
    cursorAt(v, 2);
    expect(inlineText(v)).toEqual([]);
  });

  it("is absent when inline is off", () => {
    const v = mount({ inline: false, column: true });
    focus(v);
    cursorAt(v, 4);
    expect(inlineText(v)).toEqual([]);
  });

  it("is not part of the extension when both modes are off", () => {
    expect(editorBlame({ inline: false, column: false })).toEqual([]);
  });

  it("opens the commit for a committed line only", () => {
    const onOpenCommit = vi.fn();
    const v = mountFocused({ onOpenCommit });
    cursorAt(v, 4);
    v.dom.querySelector<HTMLElement>(".cm-blameInline")!.click();
    expect(onOpenCommit).toHaveBeenCalledTimes(1);
    expect(onOpenCommit).toHaveBeenCalledWith(B);

    cursorAt(v, 6);
    v.dom.querySelector<HTMLElement>(".cm-blameInline")!.click();
    expect(onOpenCommit).toHaveBeenCalledTimes(1);
  });

  it("does nothing on click without a handler", () => {
    const v = mountFocused();
    cursorAt(v, 4);
    expect(() =>
      v.dom.querySelector<HTMLElement>(".cm-blameInline")!.click(),
    ).not.toThrow();
  });

  it("shows details on hover and removes them on leave", () => {
    const v = mountFocused({ onOpenCommit: vi.fn() });
    cursorAt(v, 4);
    const el = v.dom.querySelector<HTMLElement>(".cm-blameInline")!;
    el.dispatchEvent(new MouseEvent("mouseenter"));
    const tip = v.dom.querySelector(".cm-blameTooltip")!;
    expect(tip.textContent).toContain("bbb2222");
    expect(tip.textContent).toContain("author-bbb2222");
    expect(tip.textContent).toContain("summary bbb2222");
    expect(tip.textContent).toContain("Click to open commit");
    el.dispatchEvent(new MouseEvent("mouseleave"));
    expect(v.dom.querySelector(".cm-blameTooltip")).toBeNull();
  });

  it("omits the click hint without a handler", () => {
    const v = mountFocused();
    cursorAt(v, 4);
    v.dom
      .querySelector(".cm-blameInline")!
      .dispatchEvent(new MouseEvent("mouseenter"));
    expect(v.dom.querySelector(".cm-blameTooltip")!.textContent).not.toContain(
      "Click to open commit",
    );
  });

  it("omits the click hint on an uncommitted line even with a handler", () => {
    const onOpenCommit = vi.fn();
    const v = mountFocused({ onOpenCommit });
    cursorAt(v, 6);
    const el = v.dom.querySelector<HTMLElement>(".cm-blameInline")!;
    el.dispatchEvent(new MouseEvent("mouseenter"));
    const tip = v.dom.querySelector(".cm-blameTooltip")!;
    expect(tip.textContent).toBe("Not committed yet");
    el.click();
    expect(onOpenCommit).not.toHaveBeenCalled();
  });

  it("stops mousedown on a clickable annotation from moving the caret", () => {
    const v = mountFocused({ onOpenCommit: vi.fn() });
    cursorAt(v, 4);
    const down = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    v.dom.querySelector(".cm-blameInline")!.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
  });

  describe("tooltip lifetime", () => {
    const tip = (v: EditorView) => v.dom.querySelector(".cm-blameTooltip");

    it("hides when the cursor moves and the annotation is replaced", () => {
      const v = mountFocused();
      cursorAt(v, 4);
      v.dom
        .querySelector(".cm-blameInline")!
        .dispatchEvent(new MouseEvent("mouseenter"));
      expect(tip(v)).not.toBeNull();
      cursorAt(v, 5);
      expect(tip(v)).toBeNull();
    });

    it("hides when blame is refreshed under the pointer", () => {
      const v = mountFocused();
      cursorAt(v, 4);
      v.dom
        .querySelector(".cm-blameInline")!
        .dispatchEvent(new MouseEvent("mouseenter"));
      expect(tip(v)).not.toBeNull();
      setBlame(v, { hunks: [...BLAME.hunks] });
      expect(tip(v)).toBeNull();
    });

    it("hides when a gutter cell is rebuilt by a blame refresh", () => {
      const v = mount({ inline: false });
      v.dom
        .querySelector(
          '.cm-blameGutter .cm-gutterElement:not([style*="hidden"]) > .cm-blameCell',
        )!
        .dispatchEvent(new MouseEvent("mouseenter"));
      expect(tip(v)).not.toBeNull();
      setBlame(v, { hunks: [...BLAME.hunks] });
      expect(tip(v)).toBeNull();
    });

    it("hides when any ancestor scrolls", () => {
      const v = mountFocused();
      cursorAt(v, 4);
      v.dom
        .querySelector(".cm-blameInline")!
        .dispatchEvent(new MouseEvent("mouseenter"));
      expect(tip(v)).not.toBeNull();
      v.dom.parentElement!.dispatchEvent(new Event("scroll"));
      expect(tip(v)).toBeNull();
    });
  });
});

describe("blame column", () => {
  // The hidden spacer that sizes the column holds a cell too.
  const cells = (v: EditorView) => [
    ...v.dom.querySelectorAll(
      '.cm-blameGutter .cm-gutterElement:not([style*="hidden"]) > .cm-blameCell',
    ),
  ];

  it("shows author and date on the first line of each hunk only", () => {
    const v = mount({ inline: false });
    const first = cells(v).map((cell) => ({
      author: cell.querySelector(".cm-blameAuthor")?.textContent ?? null,
      date: cell.querySelector(".cm-blameDate")?.textContent ?? null,
    }));
    const date = new Date((SEC - 2 * 86400) * 1000);
    const iso = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
    expect(first).toEqual([
      { author: "author-aaa1111", date: iso },
      { author: null, date: null },
      { author: "author-bbb2222", date: iso },
      { author: null, date: null },
      { author: null, date: null },
      { author: "Uncommitted", date: null },
    ]);
  });

  describe("when the file has no blame", () => {
    const gutter = (v: EditorView) =>
      v.dom.querySelector<HTMLElement>(".cm-blameGutter");

    it("keeps its width while blame has not loaded yet", () => {
      const v = mount({ inline: false }, DOC, null);
      expect(gutter(v)).not.toBeNull();
      expect(gutter(v)!.querySelector(".cm-blameCell")).not.toBeNull();
      expect(v.dom.classList.contains("cm-blameNone")).toBe(false);
    });

    it("collapses the column once blame is known to be absent", () => {
      const v = mount({ inline: false }, DOC, null);
      setBlame(v, null);
      expect(v.dom.classList.contains("cm-blameNone")).toBe(true);
      expect(getComputedStyle(gutter(v)!).display).toBe("none");
      expect(cells(v)).toEqual([]);
    });

    it("treats a blame without hunks as absent", () => {
      const v = mount({ inline: false });
      setBlame(v, { hunks: [] });
      expect(v.dom.classList.contains("cm-blameNone")).toBe(true);
      expect(owners(v).every((hunk) => hunk === null)).toBe(true);
    });

    it("brings the column back when blame arrives later", () => {
      const v = mount({ inline: false }, DOC, null);
      setBlame(v, null);
      setBlame(v, BLAME);
      expect(v.dom.classList.contains("cm-blameNone")).toBe(false);
      expect(getComputedStyle(gutter(v)!).display).not.toBe("none");
      expect(cells(v).length).toBeGreaterThan(0);
    });

    it("stays collapsed through edits", () => {
      const v = mount({ inline: false }, DOC, null);
      setBlame(v, null);
      v.dispatch({ changes: { from: 0, insert: "new\n" } });
      expect(v.dom.classList.contains("cm-blameNone")).toBe(true);
    });
  });

  it("starts a new block after an inserted line splits a hunk", () => {
    const v = mount({ inline: false });
    v.dispatch({ changes: { from: v.state.doc.line(4).to, insert: "\nnew" } });
    expect(
      cells(v).map(
        (c) => c.querySelector(".cm-blameAuthor")?.textContent ?? "",
      ),
    ).toEqual([
      "author-aaa1111",
      "",
      "author-bbb2222",
      "",
      "Uncommitted",
      "author-bbb2222",
      "Uncommitted",
    ]);
  });

  it("sits before the line numbers", () => {
    const v = mount({ inline: false });
    const gutters = [...v.dom.querySelectorAll(".cm-gutters > .cm-gutter")];
    expect(gutters.map((el) => el.classList[1])).toEqual([
      "cm-blameGutter",
      "cm-lineNumbers",
    ]);
  });

  it("is absent when column is off", () => {
    const v = mount({ column: false });
    expect(v.dom.querySelector(".cm-blameGutter")).toBeNull();
  });

  it("keeps its blame when the mode is reconfigured", () => {
    const compartment = new Compartment();
    const parent = document.createElement("div");
    document.body.append(parent);
    view = new EditorView({
      state: EditorState.create({
        doc: DOC,
        extensions: compartment.of(
          editorBlame({ inline: true, column: false }),
        ),
      }),
      parent,
    });
    setBlame(view, BLAME);
    expect(view.dom.querySelector(".cm-blameGutter")).toBeNull();

    view.dispatch({
      effects: compartment.reconfigure(
        editorBlame({ inline: false, column: true }),
      ),
    });
    expect(owners(view)).toEqual([A, A, B, B, B, U]);
    expect(cells(view)).toHaveLength(6);
  });

  it("opens the commit when a committed cell is clicked, not an uncommitted one", () => {
    const onOpenCommit = vi.fn();
    const v = mount({ inline: false, onOpenCommit });
    const all = cells(v) as HTMLElement[];
    all[0].click();
    expect(onOpenCommit).toHaveBeenLastCalledWith(A);
    all[3].click();
    expect(onOpenCommit).toHaveBeenLastCalledWith(B);
    expect(onOpenCommit).toHaveBeenCalledTimes(2);
    all[5].click();
    expect(onOpenCommit).toHaveBeenCalledTimes(2);
  });

  it("reserves its width before blame arrives and renders nothing per line", () => {
    const v = mount({ inline: false }, DOC, null);
    expect(v.dom.querySelector(".cm-blameGutter")).not.toBeNull();
    expect(cells(v)).toHaveLength(0);
  });
});

describe("document ending in a newline", () => {
  // Git sees two lines here, CodeMirror three.
  const TRAILING_DOC = "one\ntwo\n";
  const TRAILING_BLAME: GitBlame = { hunks: [hunk("ccc3333", 1, 2)] };
  const C = TRAILING_BLAME.hunks[0];

  const cellTexts = (v: EditorView) =>
    [
      ...v.dom.querySelectorAll(
        '.cm-blameGutter .cm-gutterElement:not([style*="hidden"])',
      ),
    ].map((el) => el.textContent);
  const inlineTexts = (v: EditorView) =>
    [...v.dom.querySelectorAll(".cm-blameInline")].map((el) => el.textContent);

  function mountTrailing() {
    const v = mount({}, TRAILING_DOC, TRAILING_BLAME);
    focus(v);
    return v;
  }

  it("shows no Uncommitted marker on the trailing empty line", () => {
    const v = mountTrailing();
    expect(v.state.doc.lines).toBe(3);
    expect(owners(v)).toEqual([C, C, null]);
    expect(cellTexts(v)).toEqual(["author-ccc3333" + shortDateOf(C), ""]);
    expect(v.dom.textContent).not.toContain("Uncommitted");
  });

  it("shows no inline annotation on the trailing empty line", () => {
    const v = mountTrailing();
    cursorAt(v, 3);
    expect(inlineTexts(v)).toEqual([]);
    cursorAt(v, 2);
    expect(inlineTexts(v)).toHaveLength(1);
  });

  it("renders no gutter cell at all for the trailing line", () => {
    const v = mountTrailing();
    expect(
      v.dom.querySelectorAll(".cm-blameGutter .cm-blameCell"),
    ).toHaveLength(
      3, // two lines plus the hidden width spacer
    );
  });

  it("calls text typed on the trailing line Not committed yet", () => {
    const v = mountTrailing();
    v.dispatch({ changes: { from: v.state.doc.length, insert: "new" } });
    expect(owners(v)).toEqual([C, C, null]);
    expect(cellTexts(v)).toEqual([
      "author-ccc3333" + shortDateOf(C),
      "",
      "Uncommitted",
    ]);
    cursorAt(v, 3);
    expect(inlineTexts(v)).toEqual(["Not committed yet"]);
  });

  it("treats a line added at the end as new, leaving the trailing line blank", () => {
    const v = mountTrailing();
    v.dispatch({
      changes: { from: v.state.doc.line(2).to, insert: "\nthree" },
    });
    expect(v.state.doc.toString()).toBe("one\ntwo\nthree\n");
    expect(owners(v)).toEqual([C, C, null, null]);
    cursorAt(v, 3);
    expect(inlineTexts(v)).toEqual(["Not committed yet"]);
    cursorAt(v, 4);
    expect(inlineTexts(v)).toEqual([]);
  });

  it("stops treating the last line as past the end once Enter is pressed on it", () => {
    const v = mountTrailing();
    v.dispatch({ changes: { from: v.state.doc.length, insert: "\n" } });
    cursorAt(v, 3);
    expect(inlineTexts(v)).toEqual(["Not committed yet"]);
    cursorAt(v, 4);
    expect(inlineTexts(v)).toEqual(["Not committed yet"]);
  });

  it("keeps a clean file clean after an edit elsewhere", () => {
    const v = mountTrailing();
    v.dispatch({ changes: { from: 0, insert: "x" } });
    cursorAt(v, 3);
    expect(inlineTexts(v)).toEqual([]);
    expect(v.dom.textContent).not.toContain("Uncommitted");
  });
});

function shortDateOf(h: GitBlameHunk): string {
  const d = new Date(h.authorTime * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
