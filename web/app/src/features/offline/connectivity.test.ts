import { describe, expect, it } from "vitest";
import { describeConnectivity, formatLastSeen } from "./connectivity";

const ready = { status: "ready", presence: "online", blocked: null, lastFrameAt: null } as const;

describe("describeConnectivity", () => {
  it("is quiet when online with a healthy link or no desktop", () => {
    expect(describeConnectivity(true, null)).toMatchObject({ kind: "online", label: null, blocksSending: false });
    expect(describeConnectivity(true, ready)).toMatchObject({ kind: "online", label: null });
  });

  it("says offline first, whatever the link says", () => {
    expect(describeConnectivity(false, ready)).toMatchObject({ kind: "offline", label: "You are offline", blocksSending: true });
    expect(describeConnectivity(false, null).kind).toBe("offline");
  });

  it("says Reconnecting... while the link reconnects", () => {
    expect(describeConnectivity(true, { ...ready, status: "reconnecting" })).toMatchObject({ kind: "reconnecting", label: "Reconnecting...", blocksSending: true });
  });

  it("says Desktop offline with the last seen time when the relay reports the desktop gone", () => {
    expect(describeConnectivity(true, { ...ready, presence: "offline", lastFrameAt: 1000 })).toMatchObject({ kind: "desktop_offline", label: "Desktop offline", lastSeenAt: 1000 });
  });

  it("says Desktop offline when the desktop was offline at open, before the link ever became ready", () => {
    for (const status of ["connecting", "handshaking", "reconnecting", "closed"] as const) {
      expect(describeConnectivity(true, { ...ready, status, presence: "offline" })).toMatchObject({ kind: "desktop_offline", label: "Desktop offline", blocksSending: true });
    }
  });

  it("uses the directory last seen when this tab has not heard from the desktop", () => {
    expect(describeConnectivity(true, { ...ready, status: "reconnecting", presence: "offline", lastFrameAt: null }, 5000)).toMatchObject({ lastSeenAt: 5000 });
    expect(describeConnectivity(true, { ...ready, presence: "offline", lastFrameAt: 1000 }, 5000)).toMatchObject({ lastSeenAt: 1000 });
  });

  it("blocks sending in every link state but ready, without a banner for the quiet ones", () => {
    for (const status of ["idle", "connecting", "handshaking", "pending", "closed"] as const) {
      expect(describeConnectivity(true, { ...ready, status, presence: null })).toMatchObject({ kind: "online", label: null, blocksSending: true });
    }
    expect(describeConnectivity(true, ready).blocksSending).toBe(false);
  });

  it("stays quiet for a blocked link: its own screen explains it", () => {
    expect(describeConnectivity(true, { ...ready, status: "reconnecting", blocked: "revoked" }).kind).toBe("online");
  });
});

describe("formatLastSeen", () => {
  it("is coarse", () => {
    const now = 10_000_000;
    expect(formatLastSeen(now - 5_000, now)).toBe("just now");
    expect(formatLastSeen(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(formatLastSeen(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(formatLastSeen(now - 2 * 86_400_000, now)).toBe("2 d ago");
  });
});
