// @vitest-environment happy-dom
import { act, createElement, useMemo, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { currentPeriod } from "../model/period";
import { buildContributorsModel } from "../model/pipeline";
import type { ContributorsConfig } from "../model/types";
import {
  ANA,
  CAT,
  CLAUDE,
  DEPENDABOT,
  ident,
  makeCommit,
  makeRaw,
  oct,
  withConfig,
} from "../model/testFixtures";
import { IdentitiesTab } from "./IdentitiesTab";

const NOW = new Date(2026, 9, 20, 10);
const BOB = ident("Bob Stone", "bob@example.com");
const BOB_HOME = ident("Bob Stone", "bob@home.net");
const RUNNER = ident("CI Runner", "ci@build-box");

const RAW = makeRaw({
  commits: [
    makeCommit("a1", ANA, oct(1)),
    makeCommit("a2", ident("Ana", "ana@laptop.local"), oct(2)),
    makeCommit("b1", BOB, oct(3)),
    makeCommit("b2", BOB, oct(4)),
    makeCommit("b3", BOB_HOME, oct(5)),
    makeCommit("c1", CAT, oct(6)),
    makeCommit("d1", DEPENDABOT, oct(7)),
    makeCommit("x1", CLAUDE, oct(8)),
    makeCommit("r1", RUNNER, oct(9)),
    makeCommit("r2", RUNNER, oct(10)),
  ],
});

const CONFIG = withConfig({
  people: [
    {
      id: "ana",
      name: "Ana Sol",
      identities: ["ana@example.com", "ana@laptop.local"],
    },
  ],
});

let container: HTMLDivElement;
let root: Root;
const changes: ContributorsConfig[] = [];

function Harness({
  initial = CONFIG,
  merge = true,
  mailmap = [],
  save,
  locked,
}: {
  initial?: ContributorsConfig;
  merge?: boolean;
  mailmap?: { name: string; identities: string[] }[];
  save?: (next: ContributorsConfig) => Promise<void>;
  locked?: boolean;
}) {
  const [config, setConfig] = useState(initial);
  const model = useMemo(
    () =>
      buildContributorsModel({
        raw: RAW,
        config,
        mailmap,
        merge,
        period: currentPeriod("month", NOW),
        now: NOW,
      }),
    [config, merge, mailmap],
  );
  return createElement(IdentitiesTab, {
    model,
    config,
    locked,
    onChangeConfig: async (next: ContributorsConfig) => {
      await save?.(next);
      changes.push(next);
      setConfig(next);
    },
  });
}

const render = (props: Parameters<typeof Harness>[0] = {}) =>
  act(async () => {
    root.render(createElement(Harness, props));
  });

const last = () => changes[changes.length - 1];
const personOf = (config: ContributorsConfig, id: string) =>
  config.people.find((p) => p.id === id);

/** A button (or menu item) by exact text or aria-label, searched across the whole body. */
function button(text: string): HTMLButtonElement {
  const found = Array.from(
    document.body.querySelectorAll<HTMLButtonElement>("button"),
  ).find(
    (b) =>
      b.textContent?.trim() === text || b.getAttribute("aria-label") === text,
  );
  if (!found) throw new Error(`no button "${text}"`);
  return found;
}
/** A menu entry by person name (the avatar initials come first in its text). */
function menuItem(name: string): HTMLButtonElement {
  const found = Array.from(
    document.body.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
  ).find((b) => b.textContent?.endsWith(name));
  if (!found) throw new Error(`no menu item "${name}"`);
  return found;
}
const click = (el: HTMLElement) => act(async () => el.click());
const card = (id: string) =>
  container.querySelector<HTMLElement>(`[data-person="${CSS.escape(id)}"]`)!;
const nameInput = (id: string) => card(id).querySelector("input")!;

async function typeInto(input: HTMLInputElement, value: string) {
  await act(async () => {
    input.focus();
    const set = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** What `document.elementFromPoint` returns, per test. */
let hit: Element | null = null;

function pointer(target: EventTarget, type: string, x = 0, y = 0, button = 0) {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: x,
    clientY: y,
    button,
  });
  Object.defineProperty(event, "pointerId", { value: 1 });
  return act(async () => {
    target.dispatchEvent(event);
  });
}
const identityRow = (personId: string, key: string) =>
  card(personId).querySelector<HTMLElement>(`[data-identity="${key}"]`)!;

/** Presses an identity row, moves over `over`, and releases. */
async function dragIdentity(row: HTMLElement, over: Element | null) {
  await pointer(row, "pointerdown", 5, 5);
  hit = over;
  await pointer(window, "pointermove", 60, 60);
  await pointer(window, "pointerup", 60, 60);
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  changes.length = 0;
  hit = null;
  document.elementFromPoint = () => hit;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

describe("People", () => {
  it("lists people with identity and commit counts and marks agent identities", async () => {
    await render({
      initial: withConfig({
        people: [
          {
            id: "ana",
            name: "Ana Sol",
            identities: [
              "ana@example.com",
              "ana@laptop.local",
              "noreply@anthropic.com",
            ],
          },
        ],
      }),
    });
    const ana = card("ana");
    expect(nameInput("ana").value).toBe("Ana Sol");
    expect(ana.textContent).toContain("3 identities · 3 commits");
    expect(ana.textContent).toContain("ana@laptop.local");
    expect(ana.textContent).toContain("AI agent");
    expect(card("bob@example.com").textContent).toContain(
      "1 identity · 2 commits",
    );
    expect(container.textContent).toContain(
      "Saved to .monoptah/contributors.json in the repo (shared with the team).",
    );
  });

  it("commits a rename on blur", async () => {
    await render();
    await typeInto(nameInput("ana"), "Ana Maria");
    expect(changes).toHaveLength(0);
    await act(async () => nameInput("ana").blur());
    expect(personOf(last(), "ana")?.name).toBe("Ana Maria");
  });

  it("commits a rename on Enter and creates a person for an unmapped one", async () => {
    await render();
    const input = nameInput("bob@example.com");
    await typeInto(input, "Robert Stone");
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    expect(personOf(last(), "bob@example.com")).toMatchObject({
      name: "Robert Stone",
      identities: ["bob@example.com"],
    });
  });

  it("reverts on Escape without saving", async () => {
    await render();
    const input = nameInput("ana");
    await typeInto(input, "Nope");
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(input.value).toBe("Ana Sol");
    expect(changes).toHaveLength(0);
  });

  it("ignores a blank name", async () => {
    await render();
    const input = nameInput("ana");
    await typeInto(input, "   ");
    await act(async () => input.blur());
    expect(changes).toHaveLength(0);
    expect(input.value).toBe("Ana Sol");
  });

  it("moves an identity when it is dragged onto another person", async () => {
    await render();
    const row = identityRow("ana", "ana@laptop.local");
    await pointer(row, "pointerdown", 5, 5);
    hit = card("bob@example.com");
    await pointer(window, "pointermove", 60, 60);
    expect(document.querySelector("[data-identity-ghost]")?.textContent).toBe(
      "ana@laptop.local",
    );
    expect(card("bob@example.com").className).toContain("bg-accent/10");
    await pointer(window, "pointerup", 60, 60);
    expect(document.querySelector("[data-identity-ghost]")).toBeNull();
    expect(personOf(last(), "ana")?.identities).toEqual(["ana@example.com"]);
    expect(personOf(last(), "bob@example.com")?.identities).toEqual([
      "bob@example.com",
      "ana@laptop.local",
    ]);
  });

  it("does nothing when an identity is dropped on its own person", async () => {
    await render();
    await dragIdentity(identityRow("ana", "ana@example.com"), card("ana"));
    expect(changes).toHaveLength(0);
  });

  it("does nothing when released outside any person or after Escape", async () => {
    await render();
    await dragIdentity(identityRow("ana", "ana@laptop.local"), container);
    expect(changes).toHaveLength(0);

    await pointer(identityRow("ana", "ana@laptop.local"), "pointerdown", 5, 5);
    hit = card("bob@example.com");
    await pointer(window, "pointermove", 60, 60);
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await pointer(window, "pointerup", 60, 60);
    expect(changes).toHaveLength(0);
    expect(document.querySelector("[data-identity-ghost]")).toBeNull();
  });

  it("does not drag from the Move to button, so it still opens its menu", async () => {
    await render();
    const trigger = button("Move ana@laptop.local to another person");
    await pointer(trigger, "pointerdown", 5, 5);
    hit = card("bob@example.com");
    await pointer(window, "pointermove", 60, 60);
    await pointer(window, "pointerup", 60, 60);
    expect(changes).toHaveLength(0);
  });

  it("keeps the rest of a mailmap group with its person when one identity moves out", async () => {
    await render({
      mailmap: [
        {
          name: "Bob Stone",
          identities: ["bob@example.com", "bob@home.net"],
        },
      ],
    });
    expect(card("bob@example.com").textContent).toContain("2 identities");
    await click(button("Move bob@home.net to another person"));
    await click(menuItem("Ana Sol"));
    expect(personOf(last(), "ana")?.identities).toContain("bob@home.net");
    const bob = card("bob@example.com");
    expect(bob).not.toBeNull();
    expect(bob.textContent).toContain("1 identity · 2 commits");
    expect(bob.textContent).not.toContain("bob@home.net");
    expect(card("ana").textContent).toContain("bob@home.net");
  });

  it("shows identities a mailmap group adds to a config person", async () => {
    await render({
      mailmap: [
        { name: "Ana", identities: ["ana@example.com", "ana@laptop.local"] },
      ],
      initial: withConfig({
        people: [
          { id: "ana", name: "Ana Sol", identities: ["ana@example.com"] },
        ],
      }),
    });
    expect(card("ana").textContent).toContain("2 identities");
    expect(card("ana").textContent).toContain("ana@laptop.local");
  });

  it("moves an identity through the Move to menu without a pointer drag", async () => {
    await render();
    await click(button("Move ana@laptop.local to another person"));
    expect(document.body.querySelector('[role="menu"]')).not.toBeNull();
    await click(menuItem("Bob Stone"));
    expect(personOf(last(), "ana")?.identities).toEqual(["ana@example.com"]);
    expect(personOf(last(), "bob@example.com")?.identities).toContain(
      "ana@laptop.local",
    );
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
  });

  it("keeps focus on the moved identity row in the destination card", async () => {
    await render();
    await click(button("Move ana@laptop.local to another person"));
    await click(menuItem("Bob Stone"));
    const row = identityRow("bob@example.com", "ana@laptop.local");
    expect(row).not.toBeNull();
    expect(document.activeElement).toBe(row);
    expect(document.activeElement?.tagName).not.toBe("INPUT");
  });

  it("keeps focus on the identity row after Add to person too", async () => {
    await render({
      initial: withConfig({
        people: [
          { id: "ana", name: "Ana Sol", identities: ["ana@example.com"] },
        ],
      }),
    });
    await click(button("Add ci@build-box to a person"));
    await click(menuItem("Ana Sol"));
    expect(document.activeElement).toBe(identityRow("ana", "ci@build-box"));
  });

  it("does not steal focus later when the move could not be saved", async () => {
    await render({
      save: async () => {
        throw new Error("disk is full");
      },
    });
    await click(button("Move ana@laptop.local to another person"));
    await click(menuItem("Bob Stone"));
    expect(document.activeElement).not.toBe(
      container.querySelector(
        '[data-person="bob@example.com"] [data-identity]',
      ),
    );
    expect(document.activeElement?.tagName).not.toBe("INPUT");
  });

  it("merges a person into another from the person menu", async () => {
    await render();
    await click(button("Actions for Cat"));
    await click(button("Merge into..."));
    await click(menuItem("Ana Sol"));
    expect(personOf(last(), "ana")?.identities).toEqual([
      "ana@example.com",
      "ana@laptop.local",
      "cat@example.com",
    ]);
    expect(personOf(last(), "ana")?.name).toBe("Ana Sol");
  });

  it("focuses the first item when a menu opens and Escape returns focus to the trigger", async () => {
    await render();
    const trigger = button("Actions for Cat");
    await click(trigger);
    expect(document.activeElement?.textContent).toBe("Merge into...");
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("keeps focus in the menu when Merge into... opens its list, and arrows move through it", async () => {
    await render();
    await click(button("Actions for Cat"));
    await click(button("Merge into..."));
    const items = Array.from(
      document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'),
    );
    expect(items.length).toBeGreaterThan(1);
    expect(document.activeElement).toBe(items[0]);
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(items[1]);
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(items[0]);
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(items[items.length - 1]);
  });

  it("lands focus on the merged person's name after Merge into...", async () => {
    await render();
    await click(button("Actions for Cat"));
    await click(button("Merge into..."));
    await click(menuItem("Ana Sol"));
    expect(document.activeElement).toBe(nameInput("ana"));
  });

  it("removing a person ungroups its identities", async () => {
    await render();
    await click(button("Actions for Ana Sol"));
    await click(button("Remove person"));
    expect(last().people).toEqual([]);
  });

  it("cannot remove a person that only exists through observation", async () => {
    await render();
    await click(button("Actions for Cat"));
    expect(button("Remove person").disabled).toBe(true);
  });

  it("creates a person named New person and focuses its name", async () => {
    await render();
    await click(button("New person"));
    expect(last().people.map((p) => p.name)).toContain("New person");
    const created = last().people.find((p) => p.name === "New person")!;
    expect(document.activeElement).toBe(nameInput(created.id));
    expect(card(created.id).textContent).toContain("0 identities");
    expect(card(created.id).textContent).toContain("Drop an identity here");
  });

  it("shows the merge-off notice and renders raw identities read-only", async () => {
    await render({ merge: false });
    expect(container.textContent).toContain(
      "Merging is off: showing raw git identities. Turn on Merge identities in the header to apply this mapping.",
    );
    expect(container.querySelector("[data-person] input")).toBeNull();
    expect(container.querySelector('[draggable="true"]')).toBeNull();
    expect(card("ana@laptop.local")).not.toBeNull();
    expect(card("ana")).toBeNull();
    expect(button("New person").disabled).toBe(true);
    expect(container.querySelector("[data-suggestion]")).toBeNull();
    expect(() => button("Dismiss")).toThrow();
    expect(
      container.querySelector('button[aria-label^="Actions for"]'),
    ).toBeNull();
  });

  it("does not show the notice while merging is on", async () => {
    await render();
    expect(container.textContent).not.toContain("Merging is off");
  });
});

describe("Suggested merges", () => {
  it("shows the reason and identities and merges into the busiest person", async () => {
    await render();
    const item = container.querySelector("[data-suggestion]")!;
    expect(item.textContent).toContain("Same name");
    expect(item.textContent).toContain("bob@example.com");
    expect(item.textContent).toContain("bob@home.net");
    await click(button("Merge into Bob Stone"));
    expect(personOf(last(), "bob@example.com")?.identities).toEqual([
      "bob@example.com",
      "bob@home.net",
    ]);
    expect(personOf(last(), "bob@example.com")?.name).toBe("Bob Stone");
  });

  it("dismisses a suggestion and it stays gone", async () => {
    await render();
    await click(button("Dismiss"));
    expect(last().dismissedMerges).toHaveLength(1);
    expect(container.querySelector("[data-suggestion]")).toBeNull();
    expect(container.textContent).toContain("No suggestions");
  });
});

describe("Unmapped and excluded identities", () => {
  it("lists the identities that need attention with commit counts", async () => {
    await render();
    const runner = container.querySelector('[data-unmapped="ci@build-box"]')!;
    expect(runner.textContent).toContain("2 commits");
    // An ordinary author is simply their own person, mapped or not.
    for (const key of ["ana@example.com", "bob@example.com", "cat@example.com"])
      expect(container.querySelector(`[data-unmapped="${key}"]`)).toBeNull();
  });

  it("explains why the card lists what it lists", async () => {
    await render();
    const card = container.querySelector("[data-unmapped-card]")!;
    expect(card.textContent).toContain(
      "Agents and machine accounts that no person lists yet.",
    );
    expect(card.textContent).toContain("Ordinary authors are not listed");
  });

  it("excludes an identity and lists it under Excluded, then includes it again", async () => {
    await render();
    const row = container.querySelector('[data-unmapped="ci@build-box"]')!;
    await click(row.querySelector("button")!);
    expect(last().excludeIdentities).toContain("ci@build-box");
    expect(
      container.querySelector('[data-unmapped="ci@build-box"]'),
    ).toBeNull();

    expect(container.querySelector("[data-excluded]")).toBeNull();
    await click(button("Excluded (2)"));
    expect(
      container.querySelector('[data-excluded="ci@build-box"]'),
    ).not.toBeNull();
    await click(button("Include again"));
    expect(last().excludeIdentities).not.toContain("ci@build-box");
    expect(
      container.querySelector('[data-unmapped="ci@build-box"]'),
    ).not.toBeNull();
  });

  it("explains an exclusion that comes from a wildcard instead of offering to lift it", async () => {
    await render();
    await click(button("Excluded (1)"));
    const bot = container.querySelector("[data-excluded]")!;
    expect(bot.textContent).toContain("*[bot]*");
    expect(bot.textContent).toContain(
      "Edit excludeIdentities in .monoptah/contributors.json to change this.",
    );
    expect(bot.querySelector("button")).toBeNull();
  });

  it("flags agents and replaces Exclude with a note", async () => {
    await render();
    const agent = container.querySelector(
      '[data-unmapped="noreply@anthropic.com"]',
    )!;
    expect(agent.textContent).toContain("AI agent");
    expect(agent.textContent).toContain(
      "Agents are credited to the human (see How is the score calculated?)",
    );
    expect(
      Array.from(agent.querySelectorAll("button")).some(
        (b) => b.textContent === "Exclude",
      ),
    ).toBe(false);
  });

  it("adds an unmapped identity to a person through the menu", async () => {
    await render();
    await click(button("Add ci@build-box to a person"));
    await click(menuItem("Ana Sol"));
    expect(personOf(last(), "ana")?.identities).toContain("ci@build-box");
  });

  it("hides Add to person while merging is off", async () => {
    await render({ merge: false });
    expect(container.textContent).not.toContain("Add to person...");
    expect(container.textContent).toContain("Exclude");
  });
});

describe("Excluding from the People cards", () => {
  const menuItemStarting = (text: string): HTMLButtonElement => {
    const found = Array.from(
      document.body.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((b) => b.textContent?.startsWith(text));
    if (!found) throw new Error(`no menu item starting "${text}"`);
    return found;
  };
  const excluded = () => last().excludeIdentities;

  it("excludes one identity from its own menu and lists it under Excluded", async () => {
    await render();
    await click(button("Actions for identity cat@example.com"));
    await click(menuItemStarting("Exclude identity"));
    expect(excluded()).toContain("cat@example.com");
    expect(card("cat@example.com")).toBeNull();
    await click(button("Excluded (2)"));
    expect(
      container.querySelector('[data-excluded="cat@example.com"]'),
    ).not.toBeNull();
    await click(
      container
        .querySelector('[data-excluded="cat@example.com"]')!
        .querySelector("button")!,
    );
    expect(excluded()).not.toContain("cat@example.com");
  });

  it("opens with focus on the first item, and Escape returns focus to the trigger", async () => {
    await render();
    const trigger = button("Actions for identity cat@example.com");
    await click(trigger);
    expect(document.activeElement?.textContent).toContain("Exclude identity");
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("excludes every identity of a person, and says so in the menu", async () => {
    await render({
      mailmap: [
        { name: "Bob Stone", identities: ["bob@example.com", "bob@home.net"] },
      ],
    });
    await click(button("Actions for Bob Stone"));
    const item = menuItemStarting("Exclude person");
    expect(item.textContent).toContain(
      "Excludes all 2 identities from the scores.",
    );
    await click(item);
    expect(excluded()).toEqual(
      expect.arrayContaining(["bob@example.com", "bob@home.net"]),
    );
  });

  it("never excludes an agent identity: its item is disabled and a person's agents are skipped", async () => {
    await render({
      initial: withConfig({
        people: [
          {
            id: "ana",
            name: "Ana Sol",
            identities: [
              "ana@example.com",
              "ana@laptop.local",
              "noreply@anthropic.com",
            ],
          },
        ],
      }),
    });
    await click(button("Actions for identity noreply@anthropic.com"));
    expect(menuItemStarting("Exclude identity").disabled).toBe(true);
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });

    await click(button("Actions for Ana Sol"));
    const item = menuItemStarting("Exclude person");
    expect(item.textContent).toContain(
      "Excludes all 2 identities from the scores.",
    );
    await click(item);
    expect(excluded()).toEqual(
      expect.arrayContaining(["ana@example.com", "ana@laptop.local"]),
    );
    expect(excluded()).not.toContain("noreply@anthropic.com");
  });

  it("disables Exclude person for a person made only of agents", async () => {
    await render({
      initial: withConfig({
        people: [
          { id: "bot", name: "Robot", identities: ["noreply@anthropic.com"] },
        ],
      }),
    });
    await click(button("Actions for Robot"));
    expect(menuItemStarting("Exclude person").disabled).toBe(true);
  });

  it("offers no exclude menus while merging is off", async () => {
    await render({ merge: false });
    expect(
      container.querySelector('button[aria-label^="Actions for"]'),
    ).toBeNull();
  });
});

describe("saving", () => {
  it("leaves a failed save to the page instead of showing its own alert", async () => {
    await render({
      save: async () => {
        throw new Error("disk is full");
      },
    });
    await typeInto(nameInput("ana"), "Ana Maria");
    await act(async () => nameInput("ana").blur());
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });
});

describe("while the config file is unreadable", () => {
  it("disables every editing control and writes nothing", async () => {
    const save = vi.fn(async () => {});
    await render({ locked: true, save });
    // Names are plain text, so they cannot be edited, and no identity can be dragged.
    expect(container.querySelector("input")).toBeNull();
    expect(container.querySelector("[data-person-drop]")).toBeNull();
    expect(container.textContent).not.toContain("Drop an identity here");
    expect(button("New person").disabled).toBe(true);
    expect(
      container.querySelector('button[aria-label^="Actions for"]'),
    ).toBeNull();
    expect(container.querySelector('button[aria-label^="Move "]')).toBeNull();
    for (const label of ["Exclude", "Dismiss"]) {
      const buttons = Array.from(
        container.querySelectorAll<HTMLButtonElement>("button"),
      ).filter((b) => b.textContent?.trim() === label);
      expect(buttons.length).toBeGreaterThan(0);
      expect(buttons.every((b) => b.disabled)).toBe(true);
    }
    const add = button("Add ci@build-box to a person");
    expect(add.disabled).toBe(true);
    await click(add);
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(save).not.toHaveBeenCalled();
  });

  it("disables Include again for an identity excluded by a literal", async () => {
    await render({
      locked: true,
      initial: withConfig({
        ...CONFIG,
        excludeIdentities: [...CONFIG.excludeIdentities, "cat@example.com"],
      }),
    });
    await click(button("Excluded (2)"));
    expect(button("Include again").disabled).toBe(true);
  });
});

describe("person colors", () => {
  it("paints each person's avatar with the color the other tabs use", async () => {
    const base = buildContributorsModel({
      raw: RAW,
      config: CONFIG,
      mailmap: [],
      merge: true,
      period: currentPeriod("month", NOW),
      now: NOW,
    });
    // A color no hash would pick, so only the shared map can explain it.
    const model = { ...base, colors: { ...base.colors, ana: "#102030" } };
    await act(async () => {
      root.render(
        createElement(IdentitiesTab, {
          model,
          config: CONFIG,
          onChangeConfig: vi.fn(),
        }),
      );
    });
    const probe = document.createElement("span");
    probe.style.backgroundColor = "#102030";
    const avatar = card("ana").querySelector<HTMLElement>("[aria-hidden]");
    expect(avatar?.style.backgroundColor).toBe(probe.style.backgroundColor);
  });
});
