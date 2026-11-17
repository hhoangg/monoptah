import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const lib = vi.hoisted(() => ({
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
  browserSupportsWebAuthn: vi.fn(() => true),
}));
vi.mock("@simplewebauthn/browser", () => lib);

import { setApiTransport, type ApiTransport } from "@/features/accounts/api";
import { createMockState, nowSeconds, type SeedOptions } from "@/features/accounts/mock/state";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { buildInviteTree, descendantsOf } from "./tree";
import { isValidGrant } from "./UserDetail";
import { userCountText } from "./AdminHome";

const CREDENTIAL = { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} };

let restore: (() => void) | undefined;
let calls: Array<{ method: string; url: string; body: unknown }> = [];

/** An admin account by default; `fresh` opens the 5 minute window so reads do not ask for a passkey. */
function install(options: SeedOptions & { fresh?: boolean } = {}) {
  const { fresh = true, ...seed } = options;
  const state = createMockState({ signedIn: true, role: "admin", ...seed });
  if (fresh) state.reauthAt = nowSeconds();
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    calls.push({ method: init.method ?? "GET", url, body: typeof init.body === "string" ? JSON.parse(init.body) : undefined });
    return server.transport(url, init);
  };
  restore = setApiTransport(transport);
  return { state, server };
}

function mount(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

const adminCalls = () => calls.filter((call) => call.url.startsWith("/api/admin/"));

beforeEach(() => {
  calls = [];
  window.localStorage.clear();
  window.sessionStorage.clear();
  lib.startRegistration.mockResolvedValue(CREDENTIAL);
  lib.startAuthentication.mockResolvedValue(CREDENTIAL);
  lib.browserSupportsWebAuthn.mockReturnValue(true);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
  window.history.replaceState(null, "", "/");
});

describe("admin guard", () => {
  it.each(["/admin", "/admin/users", "/admin/invites"])("%s shows Not available to a non-admin and fetches no admin data", async (path) => {
    install({ role: "user" });
    mount(path);
    expect(await screen.findByRole("heading", { level: 1, name: "Not available" })).toBeInTheDocument();
    expect(screen.queryByText("Pause invites")).not.toBeInTheDocument();
    expect(adminCalls()).toHaveLength(0);
  });

  it("still handles the server's 403 for an account the client believed was an admin", async () => {
    const { server } = install();
    restore?.();
    restore = setApiTransport((url, init) =>
      url.startsWith("/api/admin/")
        ? Promise.resolve(new Response(JSON.stringify({ error: "Not allowed", code: "forbidden" }), { status: 403, headers: { "Content-Type": "application/json" } }))
        : server.transport(url, init),
    );
    mount("/admin");
    expect(await screen.findByText("This page is not available for your account.")).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Pause invites" })).not.toBeInTheDocument();
  });

  it("links to /admin from /settings for admins only", async () => {
    install();
    mount("/settings");
    expect(await screen.findByRole("link", { name: /Admin/ })).toHaveAttribute("href", "/admin");
  });

  it("has no admin link in /settings for a member", async () => {
    install({ role: "user" });
    mount("/settings");
    await screen.findByRole("heading", { level: 1, name: "Settings" });
    expect(screen.queryByRole("link", { name: /^Admin/ })).not.toBeInTheDocument();
  });
});

describe("fresh session", () => {
  it("asks to confirm first, opens no passkey prompt on load, and shows the page after confirming", async () => {
    install({ fresh: false });
    const user = userEvent.setup();
    mount("/admin");
    expect(await screen.findByRole("heading", { name: "Confirm it's you" })).toBeInTheDocument();
    expect(lib.startAuthentication).not.toHaveBeenCalled();
    expect(screen.queryByRole("switch", { name: "Pause invites" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Confirm with passkey" }));
    expect(await screen.findByRole("switch", { name: "Pause invites" })).toBeInTheDocument();
    expect(lib.startAuthentication).toHaveBeenCalledTimes(1);
    expect(calls.some((call) => call.url === "/api/auth/reauth/verify")).toBe(true);
  });

  it("shows a soft notice and no data when the passkey prompt is closed", async () => {
    install({ fresh: false });
    lib.startAuthentication.mockRejectedValue(Object.assign(new Error("closed"), { name: "NotAllowedError" }));
    const user = userEvent.setup();
    mount("/admin");
    await user.click(await screen.findByRole("button", { name: "Confirm with passkey" }));
    expect(await screen.findByText(/prompt was closed/)).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Pause invites" })).not.toBeInTheDocument();
  });

  it("re-authenticates for a write when the window ran out, then retries it", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin");
    await user.click(await screen.findByRole("switch", { name: "Pause invites" }));
    expect(state.invitesPaused).toBe(true);

    state.reauthAt = null; // the 5 minutes ran out
    calls = [];
    await user.click(screen.getByRole("switch", { name: "Pause invites" }));
    await waitFor(() => expect(state.invitesPaused).toBe(false));
    expect(calls.filter((call) => call.method === "PUT")).toHaveLength(2); // fresh_required, then the retry
    expect(lib.startAuthentication).toHaveBeenCalledTimes(1);
  });
});

describe("/admin", () => {
  it("shows the user count of the cap, the open state and the audit log", async () => {
    install();
    mount("/admin");
    expect(await screen.findByText("9 of 150")).toBeInTheDocument();
    expect(screen.getByText("Invites are open")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Pause invites" })).not.toBeChecked();
    expect(await screen.findByText("Blocked a branch (2 users)")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Block, unblock and grant invites/ })).toBeInTheDocument();
  });

  it("says No cap when maxUsers is 0", async () => {
    install({ maxUsers: 0 });
    mount("/admin");
    expect(await screen.findByText("9 (No cap)")).toBeInTheDocument();
    expect(userCountText({ invitesPaused: false, maxUsers: 0, userCount: 3 })).toBe("3 (No cap)");
  });

  it("the Pause invites switch PUTs {invitesPaused:true}, shows the paused state and writes an audit row", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin");
    await user.click(await screen.findByRole("switch", { name: "Pause invites" }));
    await screen.findByText("Invites are paused");
    expect(screen.getByRole("switch", { name: "Pause invites" })).toBeChecked();
    const put = calls.find((call) => call.method === "PUT");
    expect(put).toMatchObject({ url: "/api/admin/settings", body: { invitesPaused: true } });
    expect(state.invitesPaused).toBe(true);
    expect(await screen.findByText("Paused invites")).toBeInTheDocument();
    expect(screen.getByText("9 of 150")).toBeInTheDocument();
  });

  it("explains a reached cap", async () => {
    install({ maxUsers: "full" });
    mount("/admin");
    expect(await screen.findByText("The user cap is reached")).toBeInTheDocument();
    expect(screen.getByText("9 of 9")).toBeInTheDocument();
  });

  it("pages the audit log with Load more", async () => {
    install();
    const user = userEvent.setup();
    mount("/admin");
    await screen.findByText("Blocked a branch (2 users)");
    expect(screen.queryAllByText(/^Signed in$/)).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText("Signed in")).toBeInTheDocument();
    expect(calls.filter((call) => call.url.startsWith("/api/admin/audit"))).toHaveLength(2);
    expect(calls.some((call) => call.url.includes("cursor=10"))).toBe(true);
  });
});

describe("/admin/invites", () => {
  it("shows \"Invites are paused\" on a paused instance", async () => {
    install({ invitesPaused: true });
    mount("/admin/invites");
    expect(await screen.findByText("Invites are paused")).toBeInTheDocument();
  });

  it("shows open invites and the invite tree from invitedBy otherwise", async () => {
    install();
    mount("/admin/invites");
    expect(await screen.findByText("Invites are open")).toBeInTheDocument();
    expect(screen.queryByText("Invites are paused")).not.toBeInTheDocument();
    const tree = await screen.findByRole("list", { name: "Invite tree" });
    const ada = within(tree).getByRole("list", { name: "Invited by @dev-user" });
    expect(within(ada).getByRole("link", { name: /@ada/ })).toBeInTheDocument();
    expect(within(tree).getByRole("list", { name: "Invited by @ada" })).toBeInTheDocument();
    expect(within(tree).getAllByText("Blocked").length).toBe(2);
  });

  it("builds the tree and descendants from the users list", () => {
    const { state } = install();
    const users = [{ id: state.user.id, handle: "dev-user", displayName: "D", role: "admin" as const, status: "active" as const, invitedBy: null, blockedVia: null, invitesGranted: 3, desktopCount: 0, createdAt: 1 }, ...state.adminUsers];
    const [root] = buildInviteTree(users);
    expect(root?.user.handle).toBe("dev-user");
    expect(descendantsOf(users, "usr-ada").map((u) => u.handle).sort()).toEqual(["ben", "cleo", "dmitri"]);
  });
});

describe("/admin/users", () => {
  it("lists users with invited by, created and desktops, and opens a detail panel", async () => {
    install();
    const user = userEvent.setup();
    mount("/admin/users");
    const list = await screen.findByRole("list", { name: "Users" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(9);
    await user.click(within(list).getByRole("button", { name: /^@ada,/ }));
    const detail = await screen.findByRole("region", { name: "@ada" });
    expect(within(detail).getByText("Invited by")).toBeInTheDocument();
    expect(within(detail).getByText("@dev-user")).toBeInTheDocument();
    expect(within(detail).getByText("2 directly, 3 in total")).toBeInTheDocument();
  });

  it("blocks this user only: cascade false and the reason in the body", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin/users?user=usr-ada");
    await user.click(await screen.findByRole("button", { name: "Block @ada" }));
    const dialog = await screen.findByRole("dialog", { name: "Block @ada?" });
    expect(within(dialog).getByRole("radio", { name: /^This user only/ })).toBeChecked();
    await user.type(within(dialog).getByLabelText("Reason (optional)"), "spam");
    await user.click(within(dialog).getByRole("button", { name: "Block @ada" }));
    await waitFor(() => expect(state.adminUsers.find((u) => u.id === "usr-ada")?.status).toBe("blocked"));
    expect(calls.find((call) => call.method === "POST" && call.url === "/api/admin/users/usr-ada/block")?.body).toEqual({ cascade: false, reason: "spam" });
    expect(state.adminUsers.find((u) => u.id === "usr-ben")?.status).toBe("active");
    expect(await screen.findByText("Blocked @ada.")).toBeInTheDocument();
  });

  it("blocks a branch: cascade true, the effects are explained, descendants are blocked", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin/users?user=usr-ada");
    await user.click(await screen.findByRole("button", { name: "Block @ada" }));
    const dialog = await screen.findByRole("dialog", { name: "Block @ada?" });
    await user.click(within(dialog).getByRole("radio", { name: /This user and everyone they invited/ }));
    expect(within(dialog).getByText(/Also blocks 3 people they invited/)).toBeInTheDocument();
    expect(within(dialog).getByText(/never restores revoked tokens or grants/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Unblocking the first user does not unblock the others/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Block 4 users" }));
    await waitFor(() => expect(state.adminUsers.find((u) => u.id === "usr-dmitri")?.status).toBe("blocked"));
    expect(calls.find((call) => call.url === "/api/admin/users/usr-ada/block")?.body).toEqual({ cascade: true, reason: "" });
    expect(state.adminUsers.find((u) => u.id === "usr-ben")?.blockedVia).toBe("usr-ada");
    expect(await screen.findByText("Blocked 4 users in the branch of @ada.")).toBeInTheDocument();
  });

  it("a branch block warns about other admins in the branch and leaves the acting admin out of the count", async () => {
    const { state } = install();
    const user = userEvent.setup();
    const ben = state.adminUsers.find((u) => u.id === "usr-ben");
    if (ben) ben.role = "admin";
    // The acting admin sits inside ada's branch (and invited ada's other relatives): they must not be counted.
    const cleo = state.adminUsers.find((u) => u.id === "usr-cleo");
    if (cleo) state.adminUsers.push({ ...cleo, id: "user-dev-1", handle: "dev", displayName: "Dev User", role: "admin", invitedBy: "usr-dmitri" });
    mount("/admin/users?user=usr-ada");
    await user.click(await screen.findByRole("button", { name: "Block @ada" }));
    const dialog = await screen.findByRole("dialog", { name: "Block @ada?" });
    await user.click(within(dialog).getByRole("radio", { name: /This user and everyone they invited/ }));
    expect(within(dialog).getByText("This also blocks 1 admin")).toBeInTheDocument();
    expect(within(dialog).getByText(/@ben is an admin in this branch/)).toBeInTheDocument();
    expect(within(dialog).getByText(/You are in this branch\. You stay signed in/)).toBeInTheDocument();
    // The count never includes the acting admin.
    expect(within(dialog).getByText(/Also blocks 5 people they invited/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Block 6 users" })).toBeInTheDocument();
  });

  it("limits the reason to 500 characters", async () => {
    install();
    mount("/admin/users?user=usr-ada");
    await userEvent.setup().click(await screen.findByRole("button", { name: "Block @ada" }));
    expect(await screen.findByLabelText("Reason (optional)")).toHaveAttribute("maxlength", "500");
  });

  it("does not offer Block on your own account", async () => {
    install();
    mount("/admin/users?user=user-dev-1");
    expect(await screen.findByText("This is your account. You cannot block yourself.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Block @/ })).not.toBeInTheDocument();
  });

  it("unblock explains that descendants stay blocked and nothing is restored, then unblocks only that user", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin/users?user=usr-mallory");
    await user.click(await screen.findByRole("button", { name: "Unblock @mallory" }));
    const dialog = await screen.findByRole("dialog", { name: "Unblock @mallory?" });
    expect(within(dialog).getByText(/People they invited stay blocked until you unblock them one by one/)).toBeInTheDocument();
    expect(within(dialog).getByText(/are not restored/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Unblock" }));
    await waitFor(() => expect(state.adminUsers.find((u) => u.id === "usr-mallory")?.status).toBe("active"));
    expect(calls.some((call) => call.method === "POST" && call.url === "/api/admin/users/usr-mallory/unblock")).toBe(true);
    expect(state.adminUsers.find((u) => u.id === "usr-trent")?.status).toBe("blocked");
    expect(await screen.findByText("Unblocked @mallory. They can sign in again.")).toBeInTheDocument();
  });

  it("grants invites: 1 to 100 are sent, anything else is refused before a request", async () => {
    const { state } = install();
    const user = userEvent.setup();
    mount("/admin/users?user=usr-ben");
    const field = await screen.findByLabelText("Grant invites");
    const submit = screen.getByRole("button", { name: "Grant invites" });
    for (const bad of ["0", "101", "1.5", ""]) {
      await user.clear(field);
      if (bad) await user.type(field, bad);
      await user.click(submit);
      expect(await screen.findByText("Enter a whole number from 1 to 100.")).toBeInTheDocument();
    }
    expect(calls.filter((call) => call.url.endsWith("/invites") && call.method === "POST")).toHaveLength(0);
    await user.clear(field);
    await user.type(field, "100");
    await user.click(submit);
    expect(await screen.findByText("Granted 100 invites to @ben.")).toBeInTheDocument();
    expect(calls.find((call) => call.url === "/api/admin/users/usr-ben/invites")?.body).toEqual({ count: 100 });
    expect(state.adminUsers.find((u) => u.id === "usr-ben")?.invitesGranted).toBe(103);
    expect([0, 1, 100, 101, 1.5].map(isValidGrant)).toEqual([false, true, true, false, false]);
  });

  it("selecting a row sets ?user= so the detail can be reached and left", async () => {
    install();
    const user = userEvent.setup();
    const { router } = mount("/admin/users");
    await user.click(await screen.findByRole("button", { name: /^@eve,/ }));
    await waitFor(() => expect(router.state.location.search).toEqual({ user: "usr-eve" }));
  });
});

describe("mock admin routes mirror the Worker", () => {
  async function call(server: ReturnType<typeof createMockServer>, method: string, path: string, body?: unknown) {
    const init: RequestInit = { method, headers: { "Content-Type": "application/json", "X-Monoptah-CSRF": "1" }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) };
    const response = await server.transport(path, init);
    return { status: response.status, json: (await response.json()) as Record<string, unknown> };
  }

  it("answers 403 forbidden to a member before checking freshness, and fresh_required to a stale admin", async () => {
    const member = createMockServer({ signedIn: true, role: "user" });
    expect((await call(member, "GET", "/api/admin/users")).json.code).toBe("forbidden");
    const stale = createMockServer({ signedIn: true, role: "admin" });
    expect((await call(stale, "GET", "/api/admin/users")).json.code).toBe("fresh_required");
  });

  it("unblocking the root does not unblock descendants; blocking yourself is 409, unknown 404, bad body 400", async () => {
    const server = createMockServer({ signedIn: true, role: "admin" });
    server.state.reauthAt = nowSeconds();
    expect((await call(server, "POST", "/api/admin/users/usr-ada/block", { cascade: true, reason: "x" })).json).toEqual({ blocked: 4 });
    expect((await call(server, "POST", "/api/admin/users/usr-ada/unblock")).status).toBe(200);
    expect(server.state.adminUsers.find((u) => u.id === "usr-ben")?.status).toBe("blocked");
    expect((await call(server, "POST", "/api/admin/users/usr-ada/unblock")).status).toBe(409);
    expect((await call(server, "POST", "/api/admin/users/user-dev-1/block", { cascade: false, reason: "" })).status).toBe(409);
    expect((await call(server, "POST", "/api/admin/users/nope/block", { cascade: false, reason: "" })).status).toBe(404);
    expect((await call(server, "POST", "/api/admin/users/usr-ada/block", { cascade: "yes" })).status).toBe(400);
    expect((await call(server, "POST", "/api/admin/users/usr-ada/invites", { count: 101 })).status).toBe(400);
    expect(server.state.audit[0]?.action).toBe("user.unblock");
  });

  it("a paused or full instance refuses new invites like the Worker", async () => {
    const server = createMockServer({ signedIn: true, role: "admin", maxUsers: "full" });
    expect((await call(server, "POST", "/api/invites")).json.code).toBe("invites_paused");
    server.state.maxUsers = 0;
    expect((await call(server, "POST", "/api/invites")).status).toBe(200);
  });
});
