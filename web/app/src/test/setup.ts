import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";

// Default API transport for every test: a signed-in account served by the dev mock, so routes that
// pass the root auth guard (GET /api/me) render without a network. A test overrides it with
// `setApiTransport(...)` (and restores it itself); this hook resets it after every test.
let restoreTransport: (() => void) | undefined;
beforeEach(() => {
  restoreTransport = setApiTransport(createMockServer({ signedIn: true }).transport);
});

afterEach(() => {
  cleanup();
  restoreTransport?.();
  restoreTransport = undefined;
});

// jsdom does not implement scrolling; the router calls it for scroll restoration.
// (Skipped in the node-environment tests of build/.)
if (typeof window !== "undefined") window.scrollTo = () => {};
