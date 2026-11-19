import { QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getLinkStore } from "@/link";
import { clearLinkStores } from "@/link/store";
import { createQueryClient } from "@/lib/queryClient";
import { TranscriptOfflineNote } from "./TranscriptOfflineNote";

function setOnline(value: boolean) {
  vi.spyOn(navigator, "onLine", "get").mockReturnValue(value);
  window.dispatchEvent(new Event(value ? "online" : "offline"));
}

function mount() {
  render(
    <QueryClientProvider client={createQueryClient()}>
      <TranscriptOfflineNote desktopId="dk1" />
    </QueryClientProvider>,
  );
  return screen.getByTestId("transcript-offline");
}

afterEach(() => {
  vi.restoreAllMocks();
  clearLinkStores();
});

describe("TranscriptOfflineNote (F-42)", () => {
  it("is an empty polite live region while the desktop is reachable", () => {
    getLinkStore("dk1").setState({ status: "ready", presence: "online" });
    const region = mount();
    expect(region).toHaveAttribute("role", "status");
    expect(region).toHaveAttribute("aria-live", "polite");
    expect(region).toBeEmptyDOMElement();
  });

  it("says the saved conversation is shown when the browser is offline, and clears when it returns", () => {
    setOnline(false);
    const region = mount();
    expect(region).toHaveTextContent("You are offline. Showing the saved conversation.");
    act(() => setOnline(true));
    expect(region).toBeEmptyDOMElement();
  });

  it("says the desktop is offline with the last time it was heard from", () => {
    getLinkStore("dk1").setState({ status: "ready", presence: "offline", lastFrameAt: Date.now() - 5 * 60_000 });
    const region = mount();
    expect(region).toHaveTextContent("Your desktop is offline. Showing the saved conversation.");
    expect(region).toHaveTextContent("Last seen 5 min ago.");
  });

  it("says it is reconnecting", () => {
    getLinkStore("dk1").setState({ status: "reconnecting" });
    expect(mount()).toHaveTextContent("Reconnecting to your desktop. Showing the saved conversation.");
  });
});
