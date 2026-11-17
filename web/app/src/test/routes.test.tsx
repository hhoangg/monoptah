import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";

// One entry per route of architecture.md 3.1 (and the file names of workstreams.md 3.1):
// the concrete URL, the route shown by the stub, and the params the stub must render as text.
const CASES: ReadonlyArray<{ url: string; route: string; params: Record<string, string>; fullPath: string }> = [
  // `/` redirects (W-22): with no last place registered it lands on /desktops.
  { url: "/", route: "/desktops", params: {}, fullPath: "/" },
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

async function renderAt(url: string) {
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }) });
  render(
    <QueryClientProvider client={createQueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByTestId("route-stub");
  return router;
}

describe("route stubs", () => {
  it.each(CASES)("renders $route for $url", async ({ url, route, params }) => {
    await renderAt(url);
    const stub = screen.getByTestId("route-stub");
    expect(stub).toHaveAttribute("data-route", route);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(route);
    for (const [key, value] of Object.entries(params)) {
      expect(stub.querySelector(`[data-param="${key}"]`)).toHaveTextContent(value);
    }
    // The shared shell wraps every stub.
    expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
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
