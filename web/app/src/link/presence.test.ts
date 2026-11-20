import { describe, expect, it } from "vitest";
import type { PresenceTarget } from "@monoptah/protocol";
import { PRESENCE_HEARTBEAT_MS, startPresence, type PresenceEnv } from "./presence.ts";

type Listener = () => void;

class FakeEnv {
  visibility: "visible" | "hidden" = "visible";
  focused = true;
  readonly docListeners = new Map<string, Set<Listener>>();
  readonly winListeners = new Map<string, Set<Listener>>();
  interval: { fn: () => void; ms: number } | null = null;
  intervalsCleared = 0;

  readonly env: PresenceEnv = {
    doc: {
      get visibilityState() {
        return fake.visibility;
      },
      hasFocus: () => this.focused,
      addEventListener: (type: string, fn: unknown) => this.#add(this.docListeners, type, fn as Listener),
      removeEventListener: (type: string, fn: unknown) => this.docListeners.get(type)?.delete(fn as Listener),
    } as unknown as PresenceEnv["doc"],
    win: {
      addEventListener: (type: string, fn: unknown) => this.#add(this.winListeners, type, fn as Listener),
      removeEventListener: (type: string, fn: unknown) => this.winListeners.get(type)?.delete(fn as Listener),
    } as unknown as PresenceEnv["win"],
    setInterval: (fn, ms) => {
      this.interval = { fn, ms };
      return 1;
    },
    clearInterval: () => {
      this.interval = null;
      this.intervalsCleared += 1;
    },
  };

  #add(map: Map<string, Set<Listener>>, type: string, fn: Listener): void {
    let set = map.get(type);
    if (!set) map.set(type, (set = new Set()));
    set.add(fn);
  }
  fireDoc(type: string): void {
    for (const fn of [...(this.docListeners.get(type) ?? [])]) fn();
  }
  fireWin(type: string): void {
    for (const fn of [...(this.winListeners.get(type) ?? [])]) fn();
  }
}

let fake: FakeEnv;
const target: PresenceTarget = { kind: "session", id: "s1" };

function setup() {
  fake = new FakeEnv();
  const sent: Array<PresenceTarget | null> = [];
  const stop = startPresence({ sendPresence: (t) => (sent.push(t), true) }, target, fake.env);
  return { sent, stop };
}

describe("startPresence", () => {
  it("reports the target at once when visible and focused and repeats every 25 s", () => {
    const { sent } = setup();
    expect(sent).toEqual([target]);
    expect(fake.interval?.ms).toBe(PRESENCE_HEARTBEAT_MS);
    fake.interval?.fn();
    expect(sent).toEqual([target, target]);
  });

  it("sends a cleared state and stops the heartbeat when the page is hidden (screen lock, app switch)", () => {
    const { sent } = setup();
    fake.visibility = "hidden";
    fake.fireDoc("visibilitychange");
    expect(sent).toEqual([target, null]);
    expect(fake.interval).toBeNull();
    // Not repeated while it stays hidden.
    fake.fireWin("blur");
    expect(sent).toEqual([target, null]);
    fake.visibility = "visible";
    fake.fireDoc("visibilitychange");
    expect(sent).toEqual([target, null, target]);
    expect(fake.interval).not.toBeNull();
  });

  it("clears on blur and reports again on focus", () => {
    const { sent } = setup();
    fake.focused = false;
    fake.fireWin("blur");
    expect(sent.at(-1)).toBeNull();
    fake.focused = true;
    fake.fireWin("focus");
    expect(sent.at(-1)).toEqual(target);
  });

  it("starts cleared when the page is not visible and focused yet", () => {
    fake = new FakeEnv();
    fake.focused = false;
    const sent: Array<PresenceTarget | null> = [];
    startPresence({ sendPresence: (t) => (sent.push(t), true) }, target, fake.env);
    expect(sent).toEqual([null]);
    expect(fake.interval).toBeNull();
  });

  it("clears on pagehide and freeze even while visibilityState still says visible", () => {
    const { sent } = setup();
    fake.fireWin("pagehide");
    expect(sent).toEqual([target, null]);
    fake.fireWin("pageshow");
    expect(sent.at(-1)).toEqual(target);
    fake.fireDoc("freeze");
    expect(sent.at(-1)).toBeNull();
    fake.fireDoc("resume");
    expect(sent.at(-1)).toEqual(target);
  });

  it("sends a cleared state and removes every listener on stop", () => {
    const { sent, stop } = setup();
    stop();
    expect(sent).toEqual([target, null]);
    expect(fake.interval).toBeNull();
    for (const set of [...fake.docListeners.values(), ...fake.winListeners.values()]) expect(set.size).toBe(0);
  });
});
