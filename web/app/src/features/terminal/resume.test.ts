import { describe, expect, it } from "vitest";
import { RESUME_KEY_PREFIX, sessionResumeStore, type ResumeEntry } from "./resume";

const entry: ResumeEntry = { terminalId: "term-1", next: 42, cwd: "app/src", idleTimeoutMs: 900_000, maxDurationMs: 28_800_000, openedAt: 1_700_000_000_000 };

function memory(): Pick<Storage, "getItem" | "setItem" | "removeItem"> & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return { data, getItem: (k) => data.get(k) ?? null, setItem: (k, v) => void data.set(k, v), removeItem: (k) => void data.delete(k) };
}

describe("resume store", () => {
  it("keeps one entry per desktop under the monoptah.terminal. key and holds ids and counters only", () => {
    const storage = memory();
    const store = sessionResumeStore("desk-1", storage);
    store.save({ ...entry, output: "secret text" } as ResumeEntry);
    expect([...storage.data.keys()]).toEqual([`${RESUME_KEY_PREFIX}desk-1`]);
    expect(Object.keys(JSON.parse(storage.data.get(`${RESUME_KEY_PREFIX}desk-1`)!)).sort()).toEqual(["cwd", "idleTimeoutMs", "maxDurationMs", "next", "openedAt", "terminalId"]);
    expect(storage.data.get(`${RESUME_KEY_PREFIX}desk-1`)).not.toContain("secret");
    expect(store.load()).toEqual(entry);
    expect(sessionResumeStore("desk-2", storage).load()).toBeNull();
    store.clear();
    expect(store.load()).toBeNull();
  });
  it("reads a damaged entry as nothing", () => {
    const storage = memory();
    storage.data.set(`${RESUME_KEY_PREFIX}d`, "{not json");
    expect(sessionResumeStore("d", storage).load()).toBeNull();
    storage.data.set(`${RESUME_KEY_PREFIX}d`, JSON.stringify({ terminalId: "", next: 1 }));
    expect(sessionResumeStore("d", storage).load()).toBeNull();
    storage.data.set(`${RESUME_KEY_PREFIX}d`, JSON.stringify({ ...entry, next: -1 }));
    expect(sessionResumeStore("d", storage).load()).toBeNull();
  });
  it("survives a storage that throws and a missing storage", () => {
    const broken = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("full"); }, removeItem: () => { throw new Error("denied"); } };
    const store = sessionResumeStore("d", broken);
    expect(store.load()).toBeNull();
    expect(() => store.save(entry)).not.toThrow();
    expect(() => store.clear()).not.toThrow();
    expect(sessionResumeStore("d", null).load()).toBeNull();
  });
});
