import { KNOWN_CAPABILITIES, type KnownCapability } from "@monoptah/protocol";
import { act, render, renderHook, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { getLinkConnection, resetLinkConnections } from "@/link/connection";
import { LinkProvider } from "@/link/LinkProvider";
import { clearLinkStores, getLinkStore } from "@/link/store";
import { FEATURES, isCapabilityAvailable, useCapability } from "./capabilities";
import { RequireCapability } from "./RequireCapability";

const pending = (() => new Promise<Response>(() => undefined)) as typeof fetch;

afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
});

function wrapper(desktopId: string) {
  getLinkConnection(desktopId, { fetch: pending });
  return ({ children }: { children: ReactNode }) => <LinkProvider desktopId={desktopId}>{children}</LinkProvider>;
}

const setCapabilities = (desktopId: string, capabilities: string[]) => act(() => getLinkStore(desktopId).setState({ capabilities }));

describe("FEATURES", () => {
  it("names every known capability", () => {
    expect(Object.keys(FEATURES).sort()).toEqual([...KNOWN_CAPABILITIES].sort());
    expect(FEATURES).toEqual({
      approvals: "Approvals",
      cloud: "Cloud sessions",
      cloudStart: "Start cloud sessions",
      cloudMove: "Move sessions to the cloud",
      gitPush: "Push from Git",
      monoTools: "Mono tools",
      monoFiles: "Mono files",
      mentionedFiles: "File previews",
      mentionedFiles2: "File previews of files made by commands",
      fileTransfer: "Opening any file from the web",
      terminal: "The terminal",
      slashCommands: "The / menu",
    });
  });
});

describe("isCapabilityAvailable", () => {
  it("is true only for advertised names and ignores unknown ones", () => {
    expect(isCapabilityAvailable(["cloud", "futureThing"], "cloud")).toBe(true);
    expect(isCapabilityAvailable(["cloud", "futureThing"], "approvals")).toBe(false);
    expect(isCapabilityAvailable([], "cloud")).toBe(false);
  });
});

describe.each(KNOWN_CAPABILITIES)("capability %s", (name: KnownCapability) => {
  const feature = FEATURES[name];

  it("hides the feature when absent and shows it when present", () => {
    render(
      <RequireCapability name={name} fallback="hide">
        <p>feature body</p>
      </RequireCapability>,
      { wrapper: wrapper(`hide-${name}`) },
    );
    expect(screen.queryByText("feature body")).toBeNull();
    setCapabilities(`hide-${name}`, [name]);
    expect(screen.getByText("feature body")).toBeInTheDocument();
  });

  it("the notice names the feature and says Monoptah", () => {
    render(
      <RequireCapability name={name} fallback="notice">
        <p>feature body</p>
      </RequireCapability>,
      { wrapper: wrapper(`notice-${name}`) },
    );
    const notice = screen.getByTestId("capability-notice");
    expect(notice).toHaveTextContent(`Update Monoptah on your desktop to use ${feature}`);
    expect(notice.textContent).not.toMatch(/monocode/i);
    expect(screen.queryByText("feature body")).toBeNull();
    setCapabilities(`notice-${name}`, [name]);
    expect(screen.queryByTestId("capability-notice")).toBeNull();
    expect(screen.getByText("feature body")).toBeInTheDocument();
  });
});

describe("live capabilities", () => {
  it("an unknown capability from the desktop is ignored and does not crash", () => {
    const { result } = renderHook(() => useCapability("cloud"), { wrapper: wrapper("unk") });
    setCapabilities("unk", ["somethingNew", "cloudy"]);
    expect(result.current).toBe(false);
    setCapabilities("unk", ["somethingNew", "cloud"]);
    expect(result.current).toBe(true);
  });

  it("a state re-send with new capabilities shows and hides the feature without a reconnect", () => {
    const connection = getLinkConnection("live", { fetch: pending });
    render(
      <RequireCapability name="gitPush" fallback="hide">
        <p>push</p>
      </RequireCapability>,
      { wrapper: ({ children }) => <LinkProvider desktopId="live">{children}</LinkProvider> },
    );
    const status = connection.status;
    setCapabilities("live", ["gitPush"]);
    expect(screen.getByText("push")).toBeInTheDocument();
    setCapabilities("live", []);
    expect(screen.queryByText("push")).toBeNull();
    expect(connection.status).toBe(status);
  });
});
