import { describe, expect, it } from "vitest";
import {
  PERSON_COLORS,
  colorFor,
  colorForPerson,
  personColorMap,
} from "./palette";

describe("palette", () => {
  it("has 8 distinct hex colors", () => {
    expect(PERSON_COLORS).toHaveLength(8);
    expect(new Set(PERSON_COLORS).size).toBe(8);
    for (const c of PERSON_COLORS) expect(c).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it("picks by index and wraps around", () => {
    expect(colorFor(0)).toBe(PERSON_COLORS[0]);
    expect(colorFor(7)).toBe(PERSON_COLORS[7]);
    expect(colorFor(8)).toBe(PERSON_COLORS[0]);
    expect(colorFor(19)).toBe(PERSON_COLORS[3]);
  });

  it("wraps a negative index", () => {
    expect(colorFor(-1)).toBe(PERSON_COLORS[7]);
  });
});

describe("colorForPerson", () => {
  it("is stable for the same id and always a palette color", () => {
    const id = "ana@example.com";
    expect(colorForPerson(id)).toBe(colorForPerson(id));
    expect(PERSON_COLORS).toContain(colorForPerson(id));
    expect(PERSON_COLORS).toContain(colorForPerson(""));
  });

  it("does not depend on the order ids are asked for", () => {
    const ids = ["a@x.io", "b@x.io", "c@x.io", "p-ana"];
    const forward = ids.map(colorForPerson);
    const backward = [...ids].reverse().map(colorForPerson).reverse();
    expect(backward).toEqual(forward);
  });

  it("spreads many ids over most of the palette", () => {
    const used = new Set(
      Array.from({ length: 200 }, (_, i) => colorForPerson(`user${i}@x.io`)),
    );
    expect(used.size).toBeGreaterThanOrEqual(6);
  });
});

describe("personColorMap", () => {
  const ids = (n: number) => Array.from({ length: n }, (_, i) => `p${i}@x.io`);

  it("gives up to eight people eight different colors, for any ids", () => {
    for (let trial = 0; trial < 200; trial += 1) {
      const people = Array.from(
        { length: 8 },
        (_, i) => `user${trial}-${i}@corp.com`,
      );
      for (let n = 1; n <= 8; n += 1) {
        const map = personColorMap(people.slice(0, n));
        expect(new Set(Object.values(map)).size).toBe(n);
      }
    }
  });

  it("is the same whatever order the ids come in", () => {
    const people = ids(6);
    expect(personColorMap([...people].reverse())).toEqual(
      personColorMap(people),
    );
  });

  it("only uses palette colors and ignores duplicate ids", () => {
    const map = personColorMap(["a", "b", "a"]);
    expect(Object.keys(map).sort()).toEqual(["a", "b"]);
    for (const color of Object.values(map)) {
      expect(PERSON_COLORS).toContain(color);
    }
  });

  it("keeps the hashed color for the first person in sorted order", () => {
    const map = personColorMap(["zed@x.io", "amy@x.io"]);
    expect(map["amy@x.io"]).toBe(colorForPerson("amy@x.io"));
  });

  it("moves a person to the next free color when the preferred one is taken", () => {
    // Two ids with the same preferred color.
    const pool = ids(40);
    const clash = pool.find((a, i) =>
      pool.slice(i + 1).some((b) => colorForPerson(a) === colorForPerson(b)),
    )!;
    const other = pool.find(
      (b) => b !== clash && colorForPerson(b) === colorForPerson(clash),
    )!;
    const map = personColorMap([clash, other]);
    expect(map[clash]).not.toBe(map[other]);
    const first = [clash, other].sort()[0];
    expect(map[first]).toBe(colorForPerson(first));
  });

  it("starts reusing colors only after all eight are taken", () => {
    const map = personColorMap(ids(12));
    const counts = new Map<string, number>();
    for (const color of Object.values(map)) {
      counts.set(color, (counts.get(color) ?? 0) + 1);
    }
    expect(counts.size).toBe(8);
    expect(Math.max(...counts.values())).toBe(2);
  });

  it("does not change when someone sorted after is added", () => {
    const base = personColorMap(["a@x.io", "b@x.io", "c@x.io"]);
    const more = personColorMap(["a@x.io", "b@x.io", "c@x.io", "d@x.io"]);
    expect(more["a@x.io"]).toBe(base["a@x.io"]);
    expect(more["b@x.io"]).toBe(base["b@x.io"]);
    expect(more["c@x.io"]).toBe(base["c@x.io"]);
  });
});
