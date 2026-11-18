// Dev and test helper of the WS-T screens: an in-browser MockDesktop on an InMemoryRelay, with the link
// connection of its desktop pre-created exactly like `data/integration.test.tsx` `setup()`. Never imported
// by app code, so it never reaches the production bundle (only `preview.html` and tests use it).
import { InMemoryRelay, MockDesktop, fingerprint, generateKeyPair } from "@monoptah/mock-desktop";
import { getLinkConnection } from "@/link";
import { RelayConnection } from "@/link/relay.ts";
import { base64UrlEncode } from "@/link/base64url.ts";

export const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
export const DEVICE_ID = "22222222-2222-4222-8222-222222222222";

export interface MockLinkOptions {
  permission?: "read" | "control";
  /** Desktop capability names; default: every known one. */
  capabilities?: string[];
  /** Called with the desktop before it attaches, to seed extra data. */
  seed?: (desktop: MockDesktop) => void;
}

export interface MockLink {
  desktop: MockDesktop;
  relay: InMemoryRelay;
}

/** Creates the mock desktop, attaches it to a relay and registers the link connection for `DESKTOP_ID`. */
export function connectMockDesktop(options: MockLinkOptions = {}): MockLink {
  const relay = new InMemoryRelay();
  const deviceKey = generateKeyPair();
  const desktop = new MockDesktop({
    desktopId: DESKTOP_ID,
    name: "Example Desktop",
    version: "9.9.9",
    ...(options.capabilities ? { capabilities: options.capabilities } : {}),
  });
  resetFixtures(desktop.world);
  desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: options.permission ?? "control" });
  options.seed?.(desktop);
  desktop.attach(relay.connectDesktop());
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
    openRelay: async (_desktopId, deviceId) => new RelayConnection(relay.connectDevice(deviceId)),
    random: () => 0.5,
    channel: 7,
  });
  return { desktop, relay };
}

type World = MockDesktop["world"];
type SessionRecord = NonNullable<ReturnType<World["sessions"]["get"]>>;
type Mutable<T> = { -readonly [K in keyof T]: T[K] };

const HARNESSES = ["claude", "codex", "cursor", "grok", "opencode"] as const;
const TITLES = [
  "Fix the flaky retry test",
  "Add pagination to the sessions endpoint",
  "Refactor the config loader",
  "Investigate slow startup",
  "Write release notes",
  "Upgrade the build toolchain",
  "Handle empty responses in the client",
  "Document the plugin API",
];

// The fixtures' projects and Monos are module-level arrays shared by every MockWorld, so seeding them
// would leak into the next desktop. The first call remembers their original content; every new link
// restores it before a seed runs.
let original: { projects: Array<Record<string, unknown>>; monos: Array<Record<string, unknown>> } | null = null;
function resetFixtures(world: World): void {
  const projects = world.projects() as unknown as Array<Record<string, unknown>>;
  const monos = world.monos() as unknown as Array<Record<string, unknown>>;
  if (!original) {
    original = { projects: projects.map((p) => ({ ...p })), monos: monos.map((m) => ({ ...m })) };
    return;
  }
  projects.splice(0, projects.length, ...original.projects.map((p) => ({ ...p })));
  monos.splice(0, monos.length, ...original.monos.map((m) => ({ ...m })));
}

/** Extra realistic data on top of the fixtures: more projects, Monos and enough sessions to page and scroll. */
export function seedWorkspace(desktop: MockDesktop, options: { sessions?: number; working?: number } = {}): void {
  const { world } = desktop;
  const total = options.sessions ?? 45;
  const working = options.working ?? 2;
  const base = world.sessions.get("sess-local-1");
  if (!base) return;
  const now = Date.now();
  for (let i = 0; i < total; i++) {
    const id = `sess-seed-${i}`;
    const record: Mutable<SessionRecord> = {
      ...base,
      id,
      title: `${TITLES[i % TITLES.length]}${i >= TITLES.length ? ` (${Math.floor(i / TITLES.length) + 1})` : ""}`,
      harness: HARNESSES[i % HARNESSES.length] as string,
      branch: i % 4 === 3 ? "" : i % 3 === 0 ? "main" : `feature/task-${i}`,
      updatedAt: now - (i + 1) * 17 * 60_000,
      busy: i < working,
      turns: structuredClone(base.turns),
      approvals: [],
      script: [],
    };
    if (i === 1) record.waiting = "question";
    world.sessions.set(id, record);
  }
  const projects = world.projects() as unknown as Array<Record<string, unknown>>;
  projects.push({ id: "proj-gamma", name: "Gamma Service", path: "/example/gamma", machine: "example-machine", branch: "main", busy: true });
  const monos = world.monos() as unknown as Array<Record<string, unknown>>;
  monos[0] = { ...monos[0], lastMessage: "**Done.** I looked at the **parser** and sent the notes.", lastActiveAt: now - 5 * 60_000, unread: 2, color: "#7c5cff" };
  monos.push({ id: "mono-two", name: "Research Mono", projectIds: ["proj-beta"], unread: 0, status: "working", lastActiveAt: now - 90 * 60_000, lastMessage: "" });
}

/** The empty workspace of the F-14 empty state: no projects, Monos or sessions. */
export function emptyWorkspace(desktop: MockDesktop): void {
  (desktop.world.projects() as unknown as unknown[]).length = 0;
  (desktop.world.monos() as unknown as unknown[]).length = 0;
  desktop.world.sessions.clear();
}
