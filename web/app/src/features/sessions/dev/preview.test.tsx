// Smoke test of the preview harness entry: it reads `?to=`, sets the target before the router exists and
// renders the real router against the seeded mock desktop.
import { screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { DESKTOP_ID } from "./mockLink";

it("renders the requested route of the seeded desktop in the requested theme", async () => {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  const root = document.createElement("div");
  root.id = "root";
  document.body.replaceChildren(root);
  window.history.replaceState(null, "", `/?to=/d/${DESKTOP_ID}/p/proj-alpha&theme=dark&permission=read`);
  await import("./preview");
  expect(await screen.findByRole("heading", { level: 1, name: "Example Alpha" }, { timeout: 5000 })).toBeInTheDocument();
  expect(window.location.pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
  expect(await screen.findByTestId("read-only-chip")).toBeInTheDocument();
  expect(document.documentElement.dataset.scheme).toBe("dark");
});
