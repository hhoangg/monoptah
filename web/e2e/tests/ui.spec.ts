// The accounts flows through the UI pages. Skipped while the routes render RouteStub (WS-S W-02); the probe turns
// each test on when the real page is served. The bodies drive the pages by role and label, as the UI spec names them.
import { bootstrapInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probeUi } from "../lib/probes.ts";

const flows = ["/register", "/login", "/recover", "/admin"] as const;

test.describe("UI pages (E-01)", () => {
  for (const route of flows) {
    test(`TODO(E-01:WS-S W-02) ${route} page`, async ({ newActor }) => {
      const actor = await newActor();
      const probe = await probeUi(actor.page, route);
      if (!probe.run) console.log(`skip reason: ${route}: ${probe.reason}`);
      test.skip(!probe.run, probe.reason);

      if (route === "/register") {
        await actor.page.goto(`/register#invite=${bootstrapInvite()}`);
        await actor.page.getByLabel(/handle/i).fill("e2euiadmin");
        await actor.page.getByRole("button", { name: /create|register|passkey/i }).click();
        await expect(actor.page.getByText(/recovery/i).first()).toBeVisible();
        expect((await actor.authenticator.credentials()).length).toBe(1);
        expect((await actor.sessionCookie())?.httpOnly).toBe(true);
      } else if (route === "/login") {
        await expect(actor.page.getByRole("button", { name: /sign in|passkey|log in/i })).toBeVisible();
      } else if (route === "/recover") {
        await expect(actor.page.getByLabel(/handle/i)).toBeVisible();
        await expect(actor.page.getByLabel(/code/i)).toBeVisible();
      } else {
        // /admin needs an admin session: the guard sends everyone else to /login.
        await expect(actor.page).toHaveURL(/\/login/);
      }
    });
  }
});
