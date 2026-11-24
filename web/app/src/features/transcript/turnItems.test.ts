import { describe, expect, it } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { readItem, readItems, readMetrics } from "./turnItems";

describe("emoji picks of a text item", () => {
  const hash = "a".repeat(64);
  const pick = (at: number, extra: Record<string, unknown> = {}) => ({ at, emotion: "Happy", name: "wave", hash, format: "png", ...extra });

  it("keeps valid picks sorted by offset, stable for equal offsets, and clamped to the text", () => {
    const item = readItem({ type: "text", id: "t", text: "Hello", emoji: [pick(9, { name: "late" }), pick(2, { name: "first" }), pick(2, { name: "second" }), pick(0, { large: true })] });
    expect(item).toMatchObject({ type: "text", text: "Hello" });
    const emoji = item?.type === "text" ? item.emoji : undefined;
    expect(emoji?.map((entry) => [entry.at, entry.name])).toEqual([[0, "wave"], [2, "first"], [2, "second"], [5, "late"]]);
    expect(emoji?.[0]?.large).toBe(true);
  });

  it("drops invalid entries and leaves the field out when none is valid", () => {
    const bad = [{ at: -1, emotion: "x", name: "n", hash, format: "png" }, { at: 1, emotion: "x", name: "n", hash: "short", format: "png" }, { at: 1, emotion: "x", name: "n", hash, format: "bmp" }, "nope", null];
    expect(readItem({ type: "text", id: "t", text: "Hello", emoji: bad })).toEqual({ type: "text", id: "t", text: "Hello" });
    const mixed = readItem({ type: "text", id: "t", text: "Hello", emoji: [...bad, pick(1)] });
    expect(mixed?.type === "text" && mixed.emoji).toHaveLength(1);
    expect(readItem({ type: "text", id: "t", text: "Hello", emoji: "wave" })).toEqual({ type: "text", id: "t", text: "Hello" });
  });

  it("keeps at most 64 picks", () => {
    const item = readItem({ type: "text", id: "t", text: "x".repeat(100), emoji: Array.from({ length: 70 }, (_, index) => pick(index)) });
    expect(item?.type === "text" && item.emoji).toHaveLength(64);
  });
});

describe("readItem", () => {
  it("narrows the eight known types", () => {
    expect(readItem({ type: "user", id: "u", text: "hi", at: 5, attachments: 2 })).toEqual({ type: "user", id: "u", text: "hi", at: 5, attachments: 2 });
    expect(readItem({ type: "text", id: "t", text: "x", streaming: true })).toEqual({ type: "text", id: "t", text: "x", streaming: true });
    expect(readItem({ type: "notice", id: "n", text: "boom", error: true })).toEqual({ type: "notice", id: "n", text: "boom", error: true });
    expect(readItem({ type: "choices", id: "c", options: ["a", 3, "b"], chosen: "a" })).toEqual({ type: "choices", id: "c", options: ["a", "b"], chosen: "a" });
    expect(readItem({ type: "document", id: "d", documentId: "doc", title: "T", kind: "md" })).toMatchObject({ type: "document", documentId: "doc" });
    expect(readItem({ type: "work", id: "w", live: true, phases: [{ id: "p", kind: "run", title: "Ran", steps: [{ id: "s", label: "ls", failed: true }, { nope: 1 }] }] })).toEqual({
      type: "work",
      id: "w",
      live: true,
      phases: [{ id: "p", kind: "run", title: "Ran", steps: [{ id: "s", label: "ls", failed: true }] }],
    });
    expect(readItem({ type: "sessions", id: "s", sessions: [{ id: "x", projectId: "p", project: "P", title: "T", harness: "claude", state: "weird" }] })).toMatchObject({
      sessions: [{ state: "done" }],
    });
  });

  it("reads the header with its fold entries", () => {
    const item = readItem({
      type: "header",
      id: "h",
      live: false,
      durationMs: 1000,
      entries: [
        { type: "prose", id: "e1", text: "note" },
        { type: "phase", id: "e2", kind: "edit", title: "Edited", steps: [] },
        { type: "other", id: "e3" },
      ],
    });
    expect(item).toMatchObject({ type: "header", entries: [{ type: "prose" }, { type: "phase", kind: "edit" }] });
    expect(item?.type === "header" && item.entries).toHaveLength(2);
  });

  it("drops an unknown type and an item without an id (F-34)", () => {
    expect(readItem({ type: "hologram", id: "x" })).toBeNull();
    expect(readItem({ type: "text", text: "no id" })).toBeNull();
    expect(readItem("text")).toBeNull();
    expect(readItem(null)).toBeNull();
  });
});

describe("readItems and readMetrics", () => {
  it("caches per turn object", () => {
    const turn: Turn = { id: "t", items: [{ type: "text", id: "a", text: "x" }, { type: "hologram", id: "b" }] };
    const first = readItems(turn);
    expect(first).toHaveLength(1);
    expect(readItems(turn)).toBe(first);
  });

  it("reads token metrics when the desktop sends them", () => {
    expect(readMetrics({ id: "t", items: [], metrics: { inputTokens: 5, outputTokens: "x" } } as Turn)).toEqual({ inputTokens: 5 });
    expect(readMetrics({ id: "t", items: [] })).toBeUndefined();
  });
});
