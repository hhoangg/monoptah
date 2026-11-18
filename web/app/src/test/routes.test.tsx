import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { isPublicPath } from "@/features/accounts/authGuard";
import { createMockServer } from "@/features/accounts/mock/transport";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";

// One entry per route of architecture.md 3.1 (and the file names of workstreams.md 3.1):
// the concrete URL, the route shown by the stub, and the params the stub must render as text.
const CASES: ReadonlyArray<{ url: string; route: string; params: Record<string, string>; fullPath: string }> = [
  // `/` redirects (F-14, W-06): with no last place registered it lands on /desktops.
  // The matched route after the redirect is /desktops, so that is its full path.
  { url: "/", route: "/desktops", params: {}, fullPath: "/desktops/" },
  { url: "/login", route: "/login", params: {}, fullPath: "/login" },
  { url: "/register#invite=ABC", route: "/register", params: {}, fullPath: "/register" },
  { url: "/recover", route: "/recover", params: {}, fullPath: "/recover" },
  { url: "/install", route: "/install", params: {}, fullPath: "/install" },
  { url: "/pair/c0de", route: "/pair/:code", params: { code: "c0de" }, fullPath: "/pair/$code" },
  { url: "/desktops", route: "/desktops", params: {}, fullPath: "/desktops/" },
  { url: "/desktops/dk1", route: "/desktops/:id", params: { id: "dk1" }, fullPath: "/desktops/$id/" },
  { url: "/desktops/dk1/verify#fp=ab12", route: "/desktops/:id/verify", params: { id: "dk1" }, fullPath: "/desktops/$id/verify" },
  { url: "/d/dk1", route: "/d/:desktopId", params: { desktopId: "dk1" }, fullPath: "/d/$desktopId/" },
  {
    url: "/d/dk1/p/proj%201",
    route: "/d/:desktopId/p/:projectId",
    params: { desktopId: "dk1", projectId: "proj 1" },
    fullPath: "/d/$desktopId/p/$projectId/",
  },
  {
    url: "/d/dk1/p/p1/new",
    route: "/d/:desktopId/p/:projectId/new",
    params: { desktopId: "dk1", projectId: "p1" },
    fullPath: "/d/$desktopId/p/$projectId/new",
  },
  {
    url: "/d/dk1/p/p1/files/src/main.rs",
    route: "/d/:desktopId/p/:projectId/files/*",
    params: { desktopId: "dk1", projectId: "p1", _splat: "src/main.rs" },
    fullPath: "/d/$desktopId/p/$projectId/files/$",
  },
  {
    url: "/d/dk1/p/p1/changes/src/lib.rs",
    route: "/d/:desktopId/p/:projectId/changes/*",
    params: { desktopId: "dk1", projectId: "p1", _splat: "src/lib.rs" },
    fullPath: "/d/$desktopId/p/$projectId/changes/$",
  },
  {
    url: "/d/dk1/p/p1/git",
    route: "/d/:desktopId/p/:projectId/git",
    params: { desktopId: "dk1", projectId: "p1" },
    fullPath: "/d/$desktopId/p/$projectId/git",
  },
  {
    url: "/d/dk1/s/s9",
    route: "/d/:desktopId/s/:sessionId",
    params: { desktopId: "dk1", sessionId: "s9" },
    fullPath: "/d/$desktopId/s/$sessionId",
  },
  {
    url: "/d/dk1/monos/m1",
    route: "/d/:desktopId/monos/:monoId",
    params: { desktopId: "dk1", monoId: "m1" },
    fullPath: "/d/$desktopId/monos/$monoId/",
  },
  {
    url: "/d/dk1/monos/m1/profile",
    route: "/d/:desktopId/monos/:monoId/profile",
    params: { desktopId: "dk1", monoId: "m1" },
    fullPath: "/d/$desktopId/monos/$monoId/profile/",
  },
  {
    url: "/d/dk1/monos/m1/profile/edit",
    route: "/d/:desktopId/monos/:monoId/profile/edit",
    params: { desktopId: "dk1", monoId: "m1" },
    fullPath: "/d/$desktopId/monos/$monoId/profile/edit",
  },
  {
    url: "/d/dk1/monos/m1/profile/habit/h3",
    route: "/d/:desktopId/monos/:monoId/profile/habit/:id",
    params: { desktopId: "dk1", monoId: "m1", id: "h3" },
    fullPath: "/d/$desktopId/monos/$monoId/profile/habit/$id",
  },
  {
    url: "/d/dk1/monos/m1/profile/documents",
    route: "/d/:desktopId/monos/:monoId/profile/documents",
    params: { desktopId: "dk1", monoId: "m1" },
    fullPath: "/d/$desktopId/monos/$monoId/profile/documents",
  },
  {
    url: "/d/dk1/monos/m1/profile/file/notes.md",
    route: "/d/:desktopId/monos/:monoId/profile/file/:name",
    params: { desktopId: "dk1", monoId: "m1", name: "notes.md" },
    fullPath: "/d/$desktopId/monos/$monoId/profile/file/$name",
  },
  { url: "/d/dk1/cloud", route: "/d/:desktopId/cloud", params: { desktopId: "dk1" }, fullPath: "/d/$desktopId/cloud/" },
  {
    url: "/d/dk1/settings",
    route: "/d/:desktopId/settings",
    params: { desktopId: "dk1" },
    fullPath: "/d/$desktopId/settings/",
  },
  { url: "/settings", route: "/settings", params: {}, fullPath: "/settings/" },
  { url: "/admin", route: "/admin", params: {}, fullPath: "/admin/" },
  { url: "/admin/users", route: "/admin/users", params: {}, fullPath: "/admin/users" },
  { url: "/admin/invites", route: "/admin/invites", params: {}, fullPath: "/admin/invites" },
];

// The auth guard asks `/api/me` for every non-public route: answer it with a signed-in mock account.
// The public sign-in family is tested as a signed-out visitor (a signed-in one is sent on).
let restoreTransport: (() => void) | undefined;
afterEach(() => restoreTransport?.());

function installAccount(url: string) {
  restoreTransport?.();
  restoreTransport = setApiTransport(createMockServer({ signedIn: !isPublicPath(new URL(url, "http://x").pathname) }).transport);
}

// Routes whose stub was replaced by a real screen (W-02, W-11): they render one h1 instead of the stub.
const SCREENS: Record<string, string> = {
  "/login": "Sign in to Monoptah",
  "/register": "Create your account",
  "/recover": "Recover your account",
  "/settings": "Settings",
  "/install": "Install Monoptah",
  "/d/dk1/settings": "Desktop settings",
  "/d/:desktopId/settings": "Desktop settings",
  // W-06 (WS-T): the catalog home and the project screen. Without a connected desktop they show their loading state.
  "/d/dk1": "Workspace",
  "/d/:desktopId": "Workspace",
  "/d/dk1/p/proj%201": "Project",
  "/d/:desktopId/p/:projectId": "Project",
  // W-07 (WS-T): the session screen. Without a connected desktop its title is "Session".
  "/d/dk1/s/s9": "Session",
  "/d/:desktopId/s/:sessionId": "Session",
};

async function renderAt(url: string) {
  installAccount(url);
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  const screenTitle = SCREENS[new URL(url, "http://x").pathname];
  if (screenTitle) await screen.findByRole("heading", { level: 1, name: screenTitle });
  else await screen.findByTestId("route-stub");
  return router;
}

describe("route stubs", () => {
  it.each(CASES)("renders $route for $url", async ({ url, route, params, fullPath }) => {
    const router = await renderAt(url);
    // Every route keeps its identity: the last match is this route with these params.
    const last = router.state.matches.at(-1);
    expect(last?.fullPath).toBe(fullPath);
    for (const [key, value] of Object.entries(params)) expect(last?.params).toHaveProperty(key, value);
    if (SCREENS[route]) {
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      // The sign-in family shows no app navigation; /install keeps it and offers a way back.
      if (["/login", "/register", "/recover"].includes(new URL(url, "http://x").pathname)) expect(screen.queryByRole("navigation", { name: "Main" })).toBeNull();
      else expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
      return;
    }
    const stub = screen.getByTestId("route-stub");
    expect(stub).toHaveAttribute("data-route", route);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(route);
    for (const [key, value] of Object.entries(params)) {
      expect(stub.querySelector(`[data-param="${key}"]`)).toHaveTextContent(value);
    }
    // The shared shell wraps every stub.
    expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
  });

  it("keeps the brand a link home everywhere, and /install offers a way back into the app", async () => {
    await renderAt("/install");
    expect(screen.getByRole("link", { name: "Monoptah" })).toHaveAttribute("href", "/");
    expect(screen.getByRole("link", { name: /Back to Monoptah/ })).toHaveAttribute("href", "/");
  });

  it("shows who is signed in on /login and continues to the redirect target on request", async () => {
    restoreTransport?.();
    restoreTransport = setApiTransport(createMockServer({ signedIn: true }).transport);
    const queryClient = createQueryClient();
    const router = createAppRouter({ history: createMemoryHistory({ initialEntries: ["/login?redirect=/settings"] }), queryClient });
    render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(/Signed in as .* \(@dev-user\)/)).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/login");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("heading", { level: 1, name: "Settings" });
    expect(router.state.location.pathname).toBe("/settings");
  });

  it("offers a different passkey on /login: signs out and shows the sign-in button", async () => {
    restoreTransport?.();
    restoreTransport = setApiTransport(createMockServer({ signedIn: true }).transport);
    const queryClient = createQueryClient();
    const router = createAppRouter({ history: createMemoryHistory({ initialEntries: ["/login"] }), queryClient });
    render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    await userEvent.click(await screen.findByRole("button", { name: "Sign in with a different passkey" }));
    expect(await screen.findByRole("button", { name: /Sign in with passkey/ })).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/login");
  });

  it("covers every route file of the generated route tree", async () => {
    const router = await renderAt("/login");
    const generated = Object.values(router.routesById).map((r) => r.fullPath);
    const covered = new Set(CASES.map((c) => c.fullPath));
    // Pathless layouts (root, the d/$desktopId layout) render through their children.
    const layouts = new Set(["/", "/d/$desktopId"]);
    const missing = [...new Set(generated)].filter((p) => !covered.has(p) && !layouts.has(p));
    expect(missing).toEqual([]);
  });

  it("mounts the desktop layout with the desktop id and the app-shell passthrough", async () => {
    await renderAt("/d/dk1/s/s9");
    // The stub inside the layout is rendered through the app-shell slot default (passthrough).
    expect(screen.getByTestId("route-stub")).toHaveAttribute("data-route", "/d/:desktopId/s/:sessionId");
  });
});
