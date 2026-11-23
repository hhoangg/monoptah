import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createFakeServer, desktop } from "@/features/desktops/testServer";
import { mountApp } from "@/features/desktops/testMount";

vi.mock("@/features/accounts/webauthn", () => ({ createPasskey: vi.fn(), getPasskey: vi.fn() }));

let restore: (() => void) | undefined;
afterEach(() => restore?.());

describe("WS-M routes", () => {
  it("/desktops redirects to /settings/desktops, which renders the list screen, not the stub", async () => {
    restore = setApiTransport(createFakeServer({ desktops: [desktop()] }).transport);
    const { router } = mountApp("/desktops");
    expect(await screen.findByRole("link", { name: /^Open Studio Mac/ })).toBeInTheDocument();
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/desktops"));
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });

  it("/desktops/:id redirects to /settings/desktops/:id, which renders the desktop page for the route param", async () => {
    restore = setApiTransport(createFakeServer({ desktops: [desktop({ id: "dk9", name: "Desk Nine" })] }).transport);
    const { router } = mountApp("/desktops/dk9");
    expect(await screen.findByRole("heading", { level: 1, name: "Desk Nine" })).toBeInTheDocument();
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/desktops/dk9"));
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });
});
