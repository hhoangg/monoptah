// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  collapsedDomains,
  density,
  listWidth,
  MAIL_PREF_KEYS,
  readingPaneSide,
  selectedFolder,
  selectedMailbox,
  showImages,
  SYNC_INTERVAL_DEFAULT_SECONDS,
  syncIntervalSeconds,
} from "./mailPrefs";
import {
  loadMailEnabled,
  MAIL_ENABLED_CHANGE_EVENT,
  saveMailEnabled,
} from "../../settings/model/settings";

const cases = [
  ["selectedMailbox", selectedMailbox, null, "42"],
  ["selectedFolder", selectedFolder, null, "starred"],
  ["collapsedDomains", collapsedDomains, [], ["a.com", "b.com"]],
  ["syncIntervalSeconds", syncIntervalSeconds, 3600, 900],
  ["listWidth", listWidth, 340, 400],
  ["readingPaneSide", readingPaneSide, "right", "bottom"],
  ["density", density, "comfortable", "compact"],
  ["showImages", showImages, "ask", "never"],
] as const;

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("mail prefs", () => {
  it("keeps the monocode.mail. prefix on every key", () => {
    for (const key of Object.values(MAIL_PREF_KEYS)) {
      expect(key.startsWith("monocode.mail.")).toBe(true);
    }
  });

  it.each(cases)(
    "%s round-trips and fires its change event",
    (name, store, fallback, value) => {
      const s = store as {
        load: () => unknown;
        save: (v: never) => void;
        event: string;
      };
      expect(s.load()).toEqual(fallback);
      const listener = vi.fn();
      window.addEventListener(s.event, listener);
      s.save(value as never);
      window.removeEventListener(s.event, listener);
      expect(listener).toHaveBeenCalledTimes(1);
      expect(s.load()).toEqual(value);
      expect(localStorage.getItem(MAIL_PREF_KEYS[name])).not.toBeNull();
    },
  );

  it("uses the documented event name for each pref", () => {
    expect(
      Object.fromEntries(cases.map(([name, store]) => [name, store.event])),
    ).toEqual({
      selectedMailbox: "monocode:mail-selectedmailbox-change",
      selectedFolder: "monocode:mail-selectedfolder-change",
      collapsedDomains: "monocode:mail-collapseddomains-change",
      syncIntervalSeconds: "monocode:mail-syncintervalseconds-change",
      listWidth: "monocode:mail-listwidth-change",
      readingPaneSide: "monocode:mail-readingpaneside-change",
      density: "monocode:mail-density-change",
      showImages: "monocode:mail-showimages-change",
    });
  });

  it("returns a stable reference until the stored value changes", () => {
    collapsedDomains.save(["a.com"]);
    expect(collapsedDomains.load()).toBe(collapsedDomains.load());
  });

  it("falls back on corrupt or out-of-range values", () => {
    localStorage.setItem(MAIL_PREF_KEYS.density, "huge");
    localStorage.setItem(MAIL_PREF_KEYS.collapsedDomains, "{not json");
    localStorage.setItem(MAIL_PREF_KEYS.syncIntervalSeconds, "5");
    expect(density.load()).toBe("comfortable");
    expect(collapsedDomains.load()).toEqual([]);
    expect(syncIntervalSeconds.load()).toBe(SYNC_INTERVAL_DEFAULT_SECONDS);
  });

  it("round-trips the enabled flag under its mandated key", () => {
    expect(loadMailEnabled()).toBe(false);
    const listener = vi.fn();
    window.addEventListener(MAIL_ENABLED_CHANGE_EVENT, listener);
    saveMailEnabled(true);
    window.removeEventListener(MAIL_ENABLED_CHANGE_EVENT, listener);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(loadMailEnabled()).toBe(true);
    expect(localStorage.getItem("monocode.mail.enabled")).toBe("1");
    expect(MAIL_ENABLED_CHANGE_EVENT).toBe("monocode:mail-enabled-change");
  });
});
