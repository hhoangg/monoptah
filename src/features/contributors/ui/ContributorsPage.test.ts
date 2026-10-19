// @vitest-environment happy-dom
import { act, createElement, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));
// Each view has its own tests; here they only report what the page gave them.
vi.mock("./ContributorsView", async () => {
  const { createElement: h } = await import("react");
  return {
    ContributorsView: (props: {
      project: string;
      tabbed?: boolean;
      onStartAgentSession?: (request: unknown) => Promise<void>;
    }) =>
      h(
        "div",
        {
          "data-testid": "github-view",
          "data-project": props.project,
          "data-tabbed": String(props.tabbed),
        },
        h(
          "button",
          { onClick: () => void props.onStartAgentSession?.({ ask: true }) },
          "Start session",
        ),
      ),
  };
});
vi.mock("../../ticket-contributors/ui/TicketContributorsView", async () => {
  const { createElement: h } = await import("react");
  return {
    TicketContributorsView: (props: {
      tabbed?: boolean;
      onOpenSettings: () => void;
    }) =>
      h(
        "div",
        { "data-testid": "ticket-view", "data-tabbed": String(props.tabbed) },
        h("button", { onClick: props.onOpenSettings }, "Open Settings"),
      ),
  };
});

import { REMOTE_PROJECT_PREFIX } from "../../projects/model/recents";
import { ContributorsPage } from "./ContributorsPage";
import type { ContributorsPageTab } from "./ContributorsPage";

type PageProps = Parameters<typeof ContributorsPage>[0];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const PROJECTS = ["/work/alpha", "/work/beta", "/work/gamma"];

type Harness = Partial<
  Omit<PageProps, "tab" | "project" | "onTabChange" | "onProjectChange">
> & { tab?: ContributorsPageTab; project?: string | null };

/** Holds the tab and project the way App does, so a click shows its effect. */
function Harness({ tab = "ticket", project = null, ...rest }: Harness) {
  const [current, setCurrent] = useState({ tab, project });
  return createElement(ContributorsPage, {
    projects: PROJECTS,
    activeProject: "/work/beta",
    besideRail: true,
    compactRail: false,
    onClose: vi.fn(),
    onToggleSidebar: vi.fn(),
    onOpenSettings: vi.fn(),
    ...rest,
    tab: current.tab,
    project: current.project,
    onTabChange: (next) => setCurrent((c) => ({ ...c, tab: next })),
    onProjectChange: (next) => setCurrent((c) => ({ ...c, project: next })),
  });
}

async function render(props: Harness = {}) {
  await act(async () => root.render(createElement(Harness, props)));
}

const click = (el: Element | null | undefined) =>
  act(async () => {
    (el as HTMLElement).click();
  });
const tab = (name: string) =>
  Array.from(container.querySelectorAll<HTMLElement>('[role="tab"]')).find(
    (el) => el.textContent === name,
  );
const projectSelect = () =>
  container.querySelector<HTMLElement>('button[aria-label^="Project"]');
const option = (label: string) =>
  Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')).find(
    (el) => (el.textContent ?? "").includes(label),
  );
const githubProject = () =>
  container
    .querySelector('[data-testid="github-view"]')
    ?.getAttribute("data-project");

describe("ContributorsPage tabs", () => {
  it("offers a Ticket and a GitHub tab", async () => {
    await render();
    expect(
      Array.from(container.querySelectorAll('[role="tab"]')).map(
        (el) => el.textContent,
      ),
    ).toEqual(["Ticket", "GitHub"]);
  });

  it("opens on the Ticket tab with no project selector", async () => {
    await render();
    expect(tab("Ticket")?.getAttribute("aria-selected")).toBe("true");
    expect(tab("GitHub")?.getAttribute("aria-selected")).toBe("false");
    expect(
      container.querySelector('[data-testid="ticket-view"]'),
    ).not.toBeNull();
    expect(container.querySelector('[data-testid="github-view"]')).toBeNull();
    expect(projectSelect()).toBeNull();
  });

  it("the GitHub tab shows the project view and its selector", async () => {
    await render();
    await click(tab("GitHub"));
    expect(tab("GitHub")?.getAttribute("aria-selected")).toBe("true");
    expect(container.querySelector('[data-testid="ticket-view"]')).toBeNull();
    expect(
      container.querySelector('[data-testid="github-view"]'),
    ).not.toBeNull();
    expect(projectSelect()).not.toBeNull();
  });

  it("arrow keys move between the tabs", async () => {
    await render();
    await act(async () => {
      tab("Ticket")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
      );
    });
    expect(tab("GitHub")?.getAttribute("aria-selected")).toBe("true");
  });

  it("hands the Ticket view its settings opener", async () => {
    const onOpenSettings = vi.fn();
    await render({ onOpenSettings });
    await click(
      Array.from(container.querySelectorAll("button")).find(
        (b) => b.textContent === "Open Settings",
      ),
    );
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });
});

describe("ContributorsPage project selector", () => {
  it("defaults to the active project", async () => {
    await render({ tab: "github" });
    expect(githubProject()).toBe("/work/beta");
    expect(projectSelect()?.textContent).toContain("beta");
  });

  it("falls back to the first local project when none is active", async () => {
    await render({
      tab: "github",
      activeProject: "",
      projects: [`${REMOTE_PROJECT_PREFIX}host-1/home/dev/app`, ...PROJECTS],
    });
    expect(githubProject()).toBe("/work/alpha");
  });

  it("lists every known project", async () => {
    await render({ tab: "github" });
    await click(projectSelect());
    expect(
      Array.from(document.querySelectorAll('[role="option"]')).map(
        (el) => el.textContent,
      ),
    ).toEqual(["alpha", "beta", "gamma"]);
  });

  it("lists a remote project and shows it as chosen", async () => {
    const remote = `${REMOTE_PROJECT_PREFIX}host-1/home/dev/app`;
    await render({
      tab: "github",
      project: remote,
      projects: [...PROJECTS, remote],
    });
    expect(githubProject()).toBe(remote);
    expect(projectSelect()?.textContent).toContain("app");
  });

  it("keeps a chosen project that the rail does not list", async () => {
    await render({ tab: "github", project: "/work/elsewhere" });
    expect(githubProject()).toBe("/work/elsewhere");
    await click(projectSelect());
    expect(option("elsewhere")).toBeDefined();
  });

  it("tells two folders with the same name apart", async () => {
    await render({
      tab: "github",
      projects: ["/work/one/app", "/work/two/app"],
      activeProject: "/work/one/app",
    });
    await click(projectSelect());
    expect(
      Array.from(document.querySelectorAll('[role="option"]')).map(
        (el) => el.textContent,
      ),
    ).toEqual(["app (/work/one/app)", "app (/work/two/app)"]);
  });

  it("picking a project shows it, and it survives a switch of tab", async () => {
    await render({ tab: "github" });
    await click(projectSelect());
    await click(option("gamma"));
    expect(githubProject()).toBe("/work/gamma");
    await click(tab("Ticket"));
    await click(tab("GitHub"));
    expect(githubProject()).toBe("/work/gamma");
  });

  it("starts an agent session for the project on show", async () => {
    const onStartAgentSession = vi.fn().mockResolvedValue(undefined);
    await render({
      tab: "github",
      project: "/work/gamma",
      onStartAgentSession,
    });
    await click(
      Array.from(container.querySelectorAll("button")).find(
        (b) => b.textContent === "Start session",
      ),
    );
    expect(onStartAgentSession).toHaveBeenCalledWith("/work/gamma", {
      ask: true,
    });
  });

  it("says so when the app knows no project", async () => {
    await render({ tab: "github", projects: [], activeProject: "" });
    expect(container.querySelector('[data-testid="github-view"]')).toBeNull();
    expect(projectSelect()).toBeNull();
    expect(container.textContent).toContain("No projects yet");
  });
});

describe("ContributorsPage header", () => {
  it("draws one header of its own in the classic layout", async () => {
    await render({ tab: "github" });
    const headers = container.querySelectorAll("[data-app-view-header]");
    expect(headers).toHaveLength(1);
    expect(headers[0].textContent).toContain("Contributors");
  });

  it("leaves the header to the title bar in the new layout", async () => {
    await render({ tabbed: true, tab: "github" });
    expect(container.querySelector("[data-app-view-header]")).toBeNull();
  });

  it("never lets the views draw a second header", async () => {
    await render({ tab: "ticket" });
    expect(
      container
        .querySelector('[data-testid="ticket-view"]')
        ?.getAttribute("data-tabbed"),
    ).toBe("true");
    await click(tab("GitHub"));
    expect(
      container
        .querySelector('[data-testid="github-view"]')
        ?.getAttribute("data-tabbed"),
    ).toBe("true");
  });
});

describe("ContributorsPage Escape", () => {
  const press = (target: EventTarget = window) => {
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      target.dispatchEvent(event);
    });
    return event;
  };

  it.each(["ticket", "github"] as const)(
    "closes the page and claims the key on the %s tab",
    async (name) => {
      const onClose = vi.fn();
      await render({ tab: name, onClose });
      const event = press();
      expect(onClose).toHaveBeenCalledTimes(1);
      // App's own handler defers to a prevented Escape, so no agent is stopped.
      expect(event.defaultPrevented).toBe(true);
    },
  );

  it("leaves an Escape that something else already took", async () => {
    const onClose = vi.fn();
    await render({ onClose });
    window.addEventListener("keydown", (e) => e.preventDefault(), {
      capture: true,
      once: true,
    });
    press();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("leaves a text field first and closes on the next press", async () => {
    const onClose = vi.fn();
    await render({ onClose });
    const input = document.createElement("input");
    container.append(input);
    input.focus();
    press(input);
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).not.toBe(input);
    press();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
