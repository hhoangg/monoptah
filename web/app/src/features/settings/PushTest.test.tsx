import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.hoisted(() => ({
  hasBrowserSubscription: vi.fn(async () => true),
  getSavedPushSubscription: vi.fn(async (): Promise<{ deviceId: string; subscriptionId: string; userId: string | null } | null> => ({ deviceId: "dev_9", subscriptionId: "sub_1", userId: "u1" })),
}));
vi.mock("@/features/push/subscribe", async (importOriginal) => ({ ...(await importOriginal<object>()), ...push }));

import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createMockState } from "@/features/accounts/mock/state";
import type { MockPushTest } from "@/features/accounts/mock/state";
import { describeTestResult, PUSH_CHANGED_EVENT, PushTest, pushServiceName, TEST_EXPIRED, TEST_NO_SUBSCRIPTION, TEST_NOT_CONFIGURED, TEST_RATE_LIMITED, TEST_SENT } from "./PushTest";

let restore: (() => void) | undefined;
let bodies: unknown[] = [];

function install(pushTest: MockPushTest, subscriptions = 1) {
  const state = createMockState({ signedIn: true });
  state.pushTest = pushTest;
  for (let index = 0; index < subscriptions; index += 1) state.pushSubscriptions.set(`push_${index}`, { endpoint: `https://web.push.apple.com/${index}`, deviceId: "dev_9" });
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    if (url === "/api/push/test") bodies.push(JSON.parse(init.body as string));
    return server.transport(url, init);
  };
  restore = setApiTransport(transport);
}

beforeEach(() => {
  bodies = [];
  vi.stubGlobal("Notification", { permission: "granted" });
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: { getRegistration: () => Promise.resolve({ pushManager: { getSubscription: () => Promise.resolve({ endpoint: "https://web.push.apple.com/abc" }) } }) },
  });
  push.hasBrowserSubscription.mockResolvedValue(true);
  push.getSavedPushSubscription.mockResolvedValue({ deviceId: "dev_9", subscriptionId: "sub_1", userId: "u1" });
});
afterEach(() => {
  cleanup();
  restore?.();
  restore = undefined;
  Reflect.deleteProperty(navigator, "serviceWorker");
  vi.unstubAllGlobals();
});

async function click() {
  const button = await screen.findByRole("button", { name: "Send test notification" });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
}

describe("PushTest", () => {
  it("sends this device's server id and shows the success text", async () => {
    install("ok");
    render(<PushTest />);
    await click();
    expect(await screen.findByText(TEST_SENT)).toBeInTheDocument();
    expect(bodies).toEqual([{ deviceId: "dev_9" }]);
  });

  it("is disabled while sending", async () => {
    install("ok");
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const inner = createMockServer({ state: (() => { const s = createMockState({ signedIn: true }); s.pushSubscriptions.set("p", { endpoint: "https://x/1", deviceId: "dev_9" }); return s; })() });
    restore?.();
    restore = setApiTransport(async (url, init) => {
      if (url === "/api/push/test") await gate;
      return inner.transport(url, init);
    });
    render(<PushTest />);
    await click();
    const busy = screen.getByRole("button", { name: "Sending..." });
    expect(busy).toBeDisabled();
    release();
    expect(await screen.findByText(TEST_SENT)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send test notification" })).toBeEnabled();
  });

  it.each([
    ["gone", TEST_EXPIRED],
    ["refused", "Apple's push service refused it (HTTP 403)."],
    ["rate_limited", TEST_RATE_LIMITED],
    ["not_configured", TEST_NOT_CONFIGURED],
  ] as const)("shows the text for %s", async (mode, text) => {
    install(mode);
    render(<PushTest />);
    await click();
    expect(await screen.findByText(text)).toBeInTheDocument();
  });

  it("says there is no subscription when the server found none", async () => {
    install("ok", 0);
    render(<PushTest />);
    await click();
    expect(await screen.findByText(TEST_NO_SUBSCRIPTION)).toBeInTheDocument();
  });

  it("is disabled when the permission is not granted", async () => {
    install("ok");
    vi.stubGlobal("Notification", { permission: "default" });
    render(<PushTest />);
    expect(await screen.findByRole("button", { name: "Send test notification" })).toBeDisabled();
    await waitFor(() => expect(push.getSavedPushSubscription).not.toHaveBeenCalled());
  });

  it("is disabled when this browser has no subscription, and enables when push is turned on", async () => {
    install("ok");
    push.hasBrowserSubscription.mockResolvedValue(false);
    render(<PushTest />);
    const button = await screen.findByRole("button", { name: "Send test notification" });
    await waitFor(() => expect(push.hasBrowserSubscription).toHaveBeenCalled());
    expect(button).toBeDisabled();
    push.hasBrowserSubscription.mockResolvedValue(true);
    window.dispatchEvent(new Event(PUSH_CHANGED_EVENT));
    await waitFor(() => expect(button).toBeEnabled());
  });
});

describe("describeTestResult", () => {
  it("maps each outcome", () => {
    expect(describeTestResult({ sent: 1, results: [{ outcome: "ok", status: 201 }] }).message).toBe(TEST_SENT);
    expect(describeTestResult({ sent: 0, results: [] }).message).toBe(TEST_NO_SUBSCRIPTION);
    expect(describeTestResult({ sent: 0, results: [{ outcome: "failed", status: 0 }] }, "Apple's push service").message).toBe("Apple's push service did not answer. Try again.");
    expect(describeTestResult({ sent: 0, results: [{ outcome: "backoff", status: 0 }] }).kind).toBe("failure");
    expect(describeTestResult({ sent: 0, results: [{ outcome: "skipped_key", status: 0 }] }).kind).toBe("failure");
    expect(pushServiceName("https://fcm.googleapis.com/send/x")).toBe("Google's push service");
    expect(pushServiceName(null)).toBe("The push service");
  });
});
