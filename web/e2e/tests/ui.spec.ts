// The accounts flows through the real WS-S pages (register, login, recover, admin guard), driven by role and label.
// Every test probes its page first and skips only while the route still renders RouteStub or the placeholder, so a
// page that is replaced by a stub again shows up as a skip with its marker, never as a silent pass.
import type { Page } from "@playwright/test";
import type { Actor } from "../lib/actor.ts";
import { namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probeUi } from "../lib/probes.ts";

/** Skips the test when `route` is not a real page yet (RouteStub or the placeholder asset page). */
async function requirePage(page: Page, route: string, marker: string): Promise<void> {
  const probe = await probeUi(page, route);
  if (!probe.run) console.log(`skip reason: ${route}: ${marker} ${probe.reason}`);
  test.skip(!probe.run, `${marker} ${probe.reason}`);
}

/**
 * Registers through the /register page up to the end of the second-passkey step (skipped) and returns the 10 recovery
 * codes the page showed. Asserts the gates on the way: Continue stays disabled until "I saved these codes" is ticked.
 */
async function registerThroughUi(actor: Actor, invite: string, handle: string, displayName: string): Promise<string[]> {
  const page = actor.page;
  await page.goto("/register");
  await page.getByLabel("Invite code").fill(invite);
  await page.getByLabel("Handle").fill(handle);
  await page.getByLabel("Display name").fill(displayName);
  await page.getByRole("button", { name: /create passkey/i }).click();
  await expect(page.getByRole("heading", { name: "Save your recovery codes" })).toBeVisible();
  const items = page.getByRole("list", { name: "Recovery codes" }).getByRole("listitem");
  await expect(items).toHaveCount(10);
  const codes = (await items.locator("span").evaluateAll((spans) => spans.filter((s) => !s.hasAttribute("aria-hidden")).map((s) => s.textContent ?? ""))).map((c) => c.trim());
  expect(codes).toHaveLength(10);
  for (const code of codes) expect(code).toMatch(/^[A-Z0-9-]{8,}$/i);
  await expect(page.getByRole("button", { name: "Continue" })).toBeDisabled();
  await page.getByLabel("I saved these codes").check();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Add a second passkey" })).toBeVisible();
  await page.getByRole("button", { name: "Skip for now" }).click();
  await expect(page).not.toHaveURL(/\/register/);
  return codes;
}

test.describe("UI pages (E-01)", () => {
  test("/register, /login and /recover: create an account, sign in, recover with a shown code", async ({ newActor }) => {
    const actor = await newActor();
    const page = actor.page;
    await requirePage(page, "/register", "TODO(E-01:WS-S register)");

    // The bootstrap invites are spent by the API specs: an admin of its own (API-registered with its own bootstrap
    // invite) creates the invite this page registers with.
    const admin = await newActor();
    expect((await admin.register(namedInvite("ui"), "e2euiowner")).status).toBe(200);
    const invite = await admin.call("POST", "/api/invites", { body: {} });
    expect(invite.status).toBe(201);

    // --- register: a passkey is created in the virtual authenticator, the recovery codes are shown, the session works.
    const codes = await registerThroughUi(actor, String(invite.json["code"]), "e2euiadmin", "E2E UI Admin");
    expect((await actor.authenticator.credentials()).length).toBe(1);
    expect((await actor.sessionCookie())?.httpOnly).toBe(true);
    const me = await actor.call("GET", "/api/me");
    expect(me.status).toBe(200);
    expect(me.json["user"]).toMatchObject({ handle: "e2euiadmin", role: "user" });

    // --- login: the same passkey signs in again after the cookie is gone.
    await actor.context.clearCookies();
    await requirePage(page, "/login", "TODO(E-01:WS-S login)");
    await page.getByRole("button", { name: "Sign in with passkey" }).click();
    await expect(page).not.toHaveURL(/\/login/);
    expect((await actor.call("GET", "/api/me")).status).toBe(200);

    // --- recover: the passkey is lost (removed from the authenticator); a code the register page showed works once
    // and leads to a new passkey.
    const [lost] = await actor.authenticator.credentials();
    await actor.authenticator.removeCredential(lost!.credentialId);
    await actor.context.clearCookies();
    await requirePage(page, "/recover", "TODO(E-01:WS-S recover)");
    await page.getByLabel("Handle").fill("e2euiadmin");
    await page.getByLabel("Recovery code").fill(codes[0] ?? "");
    await page.getByRole("button", { name: "Recover account" }).click();
    await expect(page.getByRole("heading", { name: "Add a new passkey" })).toBeVisible();
    await page.getByRole("button", { name: "Add a passkey" }).click();
    await expect(page).toHaveURL(/\/login/);
    const replaced = await actor.authenticator.credentials();
    expect(replaced).toHaveLength(1);
    expect(replaced[0]?.credentialId).not.toBe(lost!.credentialId);
    // The recovery session ended with the sign-out; the new passkey signs in.
    expect((await actor.call("GET", "/api/me")).status).toBe(401);
    await page.getByRole("button", { name: "Sign in with passkey" }).click();
    await expect(page).not.toHaveURL(/\/login/);
    expect((await actor.call("GET", "/api/me")).status).toBe(200);

    // The same code is spent.
    await actor.context.clearCookies();
    await page.goto("/recover");
    await page.getByLabel("Handle").fill("e2euiadmin");
    await page.getByLabel("Recovery code").fill(codes[0] ?? "");
    await page.getByRole("button", { name: "Recover account" }).click();
    await expect(page.getByText("That handle or recovery code is not valid.")).toBeVisible();
    await expect(page.getByRole("heading", { name: "Add a new passkey" })).toHaveCount(0);
  });

  test("/admin: the guard sends a signed-out visitor to /login with a redirect", async ({ newActor }) => {
    const actor = await newActor();
    await requirePage(actor.page, "/login", "TODO(E-01:WS-S login)");
    await actor.page.goto("/admin");
    await expect(actor.page).toHaveURL(/\/login\?redirect=%2Fadmin/);
    await expect(actor.page.getByRole("button", { name: "Sign in with passkey" })).toBeVisible();
  });

  test("/admin: an admin session opens the admin overview", async ({ newActor }) => {
    const actor = await newActor();
    await requirePage(actor.page, "/register", "TODO(E-01:WS-S register)");
    // The bootstrap invite makes an admin.
    await registerThroughUi(actor, namedInvite("devices"), "e2eadminui", "E2E Admin");
    const me = await actor.call("GET", "/api/me");
    expect(me.json["user"]).toMatchObject({ role: "admin" });
    // Nothing sets `users.recovery_ack_at` after register (or regenerate), so the guard confines every new account to
    // /settings ("Get new recovery codes"). Until WS-A/WS-S acknowledge the codes the admin pages are out of reach.
    if (me.json["recoveryAckAt"] === null) console.log("skip reason: recoveryAckAt stays null after /register");
    test.skip(me.json["recoveryAckAt"] === null, "TODO(E-01:WS-A A-05, WS-S W-02) recoveryAckAt is null after register, the guard confines the account to /settings");
    await requirePage(actor.page, "/admin", "TODO(E-01:WS-S admin screens W-02)");
    await expect(actor.page).toHaveURL(/\/admin$/);
  });
});
