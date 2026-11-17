import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

// jsdom does not implement scrolling; the router calls it for scroll restoration.
// (Skipped in the node-environment tests of build/.)
if (typeof window !== "undefined") window.scrollTo = () => {};
