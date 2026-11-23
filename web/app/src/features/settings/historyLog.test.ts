import { afterEach, describe, expect, it } from "vitest";
import { entryBeforeSettings, recordHistoryEntry } from "./historyLog";

afterEach(() => sessionStorage.clear());

function visit(...paths: string[]) {
  paths.forEach((path, index) => recordHistoryEntry(index, path));
}

describe("history log", () => {
  it("keeps the entries a Back jump passed over, so a later Forward jump still finds them", () => {
    visit("/d/A", "/d/A/p/x", "/settings/profile");
    recordHistoryEntry(0, "/d/A");
    recordHistoryEntry(2, "/settings/profile");
    expect(entryBeforeSettings(2)).toBe(1);
  });

  it("stops at an entry that was never logged instead of skipping past it", () => {
    recordHistoryEntry(0, "/d/A");
    recordHistoryEntry(2, "/settings/profile");
    expect(entryBeforeSettings(2)).toBeNull();
  });

  it("skips settings, sign-in and one-shot pair pages", () => {
    visit("/d/A", "/pair/CODE", "/login", "/settings/desktops", "/settings/profile");
    expect(entryBeforeSettings(4)).toBe(0);
  });

  it("skips the Pair a desktop page itself, exact path or with a trailing slash", () => {
    visit("/d/A", "/pair", "/settings/desktops", "/pair/", "/settings/profile");
    expect(entryBeforeSettings(4)).toBe(0);
  });

  it("does not skip a page that only starts like a skipped one", () => {
    visit("/d/A", "/pairing", "/settings/profile");
    expect(entryBeforeSettings(2)).toBe(1);
  });
});
