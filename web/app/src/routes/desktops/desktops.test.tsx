import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createFakeServer, desktop } from "@/features/desktops/testServer";
import { mountApp } from "@/features/desktops/testMount";

vi.mock("@/features/accounts/webauthn", () => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));

let restore: (() => void) | undefined;
afterEach(() => restore?.());

describe("WS-M routes", () => {
  it("/desktops renders the list screen, not the stub", async () => {
    restore = setApiTransport(createFakeServer({ desktops: [desktop()] }).transport);
    mountApp("/desktops");
    expect(await screen.findByRole("link", { name: "Open Studio Mac" })).toBeInTheDocument();
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });

  it("/desktops/:id renders the detail screen for the route param", async () => {
    restore = setApiTransport(createFakeServer({ desktops: [desktop({ id: "dk9", name: "Desk Nine" })] }).transport);
    mountApp("/desktops/dk9");
    expect(await screen.findByRole("heading", { level: 1, name: "Desk Nine" })).toBeInTheDocument();
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });
});
