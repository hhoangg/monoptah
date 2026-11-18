import { beforeEach, describe, expect, it } from "vitest";
import { markGrantRequested, readLastUsed, sortByLastUsed, touchLastUsed, wasGrantRequested } from "./storage";

beforeEach(() => localStorage.clear());

describe("desktop storage records", () => {
  it("remembers a grant request per desktop and device", () => {
    expect(wasGrantRequested("d1", "dev1")).toBe(false);
    markGrantRequested("d1", "dev1");
    expect(wasGrantRequested("d1", "dev1")).toBe(true);
    expect(wasGrantRequested("d2", "dev1")).toBe(false);
    expect(wasGrantRequested("d1", "dev2")).toBe(false);
  });

  it("keeps the last used order, newest first, without duplicates", () => {
    touchLastUsed("a");
    touchLastUsed("b");
    touchLastUsed("a");
    expect(readLastUsed()).toEqual(["a", "b"]);
  });

  it("ignores a corrupt stored order", () => {
    localStorage.setItem("monoptah.desktops.lastUsed", "{not json");
    expect(readLastUsed()).toEqual([]);
    localStorage.setItem("monoptah.desktops.lastUsed", JSON.stringify([1, "x", null]));
    expect(readLastUsed()).toEqual(["x"]);
  });

  it("sorts used desktops first and keeps the given order for the rest", () => {
    const items = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }];
    expect(sortByLastUsed(items, ["c", "a"]).map((item) => item.id)).toEqual(["c", "a", "b", "d"]);
  });
});
