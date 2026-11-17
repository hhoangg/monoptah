// The data layer over the real link client and the mock desktop: cloud and approvals reach the typed
// hooks from reads and watch frames, a reconnect reopens the watch, a send is never re-sent.
import { QueryClientProvider } from "@tanstack/react-query";
import { act, render } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InMemoryRelay, MockDesktop, fingerprint, generateKeyPair } from "@monoptah/mock-desktop";
import { base64UrlEncode } from "@/link/base64url.ts";
import { getLinkConnection, resetLinkConnections, LinkProvider } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { RelayConnection } from "@/link/relay.ts";
import { createQueryClient } from "@/lib/queryClient";
import { useApprovals, useCloud, useConversation, type UseConversationResult } from "./conversation.ts";
import { useCatalog, useLinkQuery } from "./queries.ts";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "./persist.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const DEVICE_ID = "22222222-2222-4222-8222-222222222222";

let relay: InMemoryRelay;
let desktop: MockDesktop;
let queryClient: ReturnType<typeof createQueryClient>;
let previous: PersistAdapter;
let conversation: UseConversationResult | null;
let opens: number;

function Screen({ id }: { id: string }) {
  const cloud = useCloud(id);
  const approvals = useApprovals(id);
  const current = useConversation(id);
  useEffect(() => {
    conversation = current;
  });
  const catalog = useCatalog();
  // Mounted while the link is still connecting: its first fetch fails with `not_ready` (I3).
  const sessions = useLinkQuery("sessions.list", { projectId: "proj-alpha" });
  return (
    <div>
      <p data-testid="cloud">{cloud ? `${cloud.kind}:${cloud.state}:${cloud.startup?.current ?? ""}` : "none"}</p>
      <p data-testid="approvals">{approvals.map((a) => a.label).join("|") || "none"}</p>
      <p data-testid="late">{sessions.status}</p>
      <p data-testid="sessions">{catalog.data ? String(catalog.data.sessions.length) : "no catalog"}</p>
    </div>
  );
}

function setup(): void {
  relay = new InMemoryRelay();
  const deviceKey = generateKeyPair();
  desktop = new MockDesktop({ desktopId: DESKTOP_ID, name: "Example Desktop", version: "9.9.9" });
  desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: "control" });
  desktop.attach(relay.connectDesktop());
  opens = 0;
  const directory = {
    id: DESKTOP_ID,
    name: "Example Desktop",
    color: null,
    online: true,
    pendingCount: 0,
    os: null,
    appVersion: null,
    lastSeenAt: null,
    publicKey: base64UrlEncode(desktop.publicKey),
    fingerprint: fingerprint(desktop.publicKey),
  };
  getLinkConnection(DESKTOP_ID, {
    ensureDevice: async () => ({ deviceId: DEVICE_ID, publicKey: deviceKey.publicKey, fingerprint: fingerprint(deviceKey.publicKey) }),
    getDeviceKeyPair: async () => deviceKey,
    fetch: (async () => new Response(JSON.stringify(directory), { status: 200, headers: { "Content-Type": "application/json" } })) as typeof fetch,
    checkPin: async () => "match",
    openRelay: async (_desktopId, deviceId) => {
      opens += 1;
      return new RelayConnection(relay.connectDevice(deviceId));
    },
    random: () => 0.5,
    channel: 7,
  });
}

async function settle(ms = 0): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
    await vi.advanceTimersByTimeAsync(0);
    await relay.settled();
    await vi.advanceTimersByTimeAsync(0);
  });
}

function mount(id: string) {
  return render(
    <QueryClientProvider client={queryClient}>
      <LinkProvider desktopId={DESKTOP_ID}>
        <Screen id={id} />
      </LinkProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  queryClient = createQueryClient();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  setup();
});
afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  vi.useRealTimers();
});

describe("typed hooks over the mock desktop", () => {
  it("a useLinkQuery mounted before the link is ready fetches when it becomes ready (I3)", async () => {
    const view = mount("sess-local-approval");
    // The first fetch runs at mount, before the handshake is done, and fails with not_ready.
    await settle();
    await settle();
    expect(getLinkConnection(DESKTOP_ID).status).toBe("ready");
    expect(view.getByTestId("late")).toHaveTextContent("success");
  });

  it("useApprovals shows the pending approval from the first frame and an empty array clears it; the catalog loads", async () => {
    const view = mount("sess-local-approval");
    await settle();
    await settle();
    expect(view.getByTestId("approvals")).toHaveTextContent("Run example command");
    expect(view.getByTestId("cloud")).toHaveTextContent("none");
    expect(Number(view.getByTestId("sessions").textContent)).toBeGreaterThan(0);

    await act(async () => {
      await getLinkConnection(DESKTOP_ID).request("session.answerApproval", { conversationId: "sess-local-approval", requestId: 41, decision: "allow" });
    });
    await settle();
    expect(view.getByTestId("approvals")).toHaveTextContent("none");
  });

  it("useCloud follows the startup steps frame by frame and keeps the field when a frame does not carry it", async () => {
    const view = mount("sess-codespace-starting");
    await settle();
    await settle();
    expect(view.getByTestId("cloud")).toHaveTextContent("codespace:starting:provision");
    desktop.advance("sess-codespace-starting");
    await settle();
    expect(view.getByTestId("cloud")).toHaveTextContent("codespace:starting:buildContainer");
    while (desktop.advance("sess-codespace-starting")) await settle();
    expect(view.getByTestId("cloud")).toHaveTextContent("codespace:available:");
    // A catalog refetch does not touch it.
    await settle(5_000);
    expect(view.getByTestId("cloud")).toHaveTextContent("codespace:available:");
    // A real watch frame without a `cloud` field (a new turn) keeps it too.
    const turnsBefore = conversation!.state!.turns.length;
    await act(async () => {
      await getLinkConnection(DESKTOP_ID).request("conversation.send", { conversationId: "sess-codespace-starting", text: "a frame without cloud" });
    });
    await settle();
    expect(conversation!.state!.turns.length).toBeGreaterThan(turnsBefore);
    expect(view.getByTestId("cloud")).toHaveTextContent("codespace:available:");
  });

  it("a reconnect reopens the watch, re-reads the conversation and keeps cloud and approvals; a send is not re-sent", async () => {
    const view = mount("sess-local-approval");
    await settle();
    await settle();
    expect(view.getByTestId("approvals")).toHaveTextContent("Run example command");
    const connection = getLinkConnection(DESKTOP_ID);
    const sends = vi.spyOn(connection, "request");

    // The desktop link drops. A send while it is down is "Not sent" and nothing sends it later.
    relay.revokeDevice(DEVICE_ID, 1012);
    await settle();
    expect(connection.status).not.toBe("ready");
    let result: Awaited<ReturnType<UseConversationResult["send"]>> | undefined;
    await act(async () => {
      result = await conversation!.send("sent while down");
    });
    expect(result).toEqual({ sent: false, error: "not_ready" });
    await settle();
    expect(conversation!.state?.optimistic[0]).toMatchObject({ text: "sent while down", status: "not_sent" });

    await settle(60_000);
    await settle();
    expect(connection.status).toBe("ready");
    expect(opens).toBe(2);
    const sendCalls = () => sends.mock.calls.filter((call) => call[0] === "conversation.send").length;
    expect(sendCalls()).toBe(1);
    expect(sends.mock.calls.some((call) => call[0] === "conversation.read")).toBe(true);
    expect(conversation!.state?.optimistic[0]?.status).toBe("not_sent");
    expect(view.getByTestId("approvals")).toHaveTextContent("Run example command");

    // Only the user retries it.
    await act(async () => {
      result = await conversation!.retry("pending:1");
    });
    await settle();
    expect(result).toEqual({ sent: true });
    expect(sendCalls()).toBe(2);
    expect(conversation!.state?.optimistic).toEqual([]);
  });
});
