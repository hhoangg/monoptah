// A 429 from the Worker's per-session limit must never crash the app: the first /api/me calls answer 429 with
// Retry-After: 3, the app shows the small "Too many requests" notice (not the error screen), waits, retries, and loads.
// A visitor without a session is used so the spec needs no invite of its own: after the retries /api/me answers 401
// and the app lands on the sign-in screen, which proves the root auth guard recovered.
import { test, expect } from "../lib/fixtures.ts";

const LIMITED_CALLS = 2;

test("consecutive 429s on /api/me show a retrying notice, not the error screen, and the app loads", async ({ newActor }) => {
  const actor = await newActor();
  const page = actor.page;

  let meCalls = 0;
  await page.route("**/api/me", async (route) => {
    if (meCalls++ < LIMITED_CALLS) {
      await route.fulfill({
        status: 429,
        headers: { "Retry-After": "3", "Content-Type": "application/json" },
        body: JSON.stringify({ error: "Too many requests", code: "rate_limited" }),
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/settings/profile");
  // Each wait lasts 3 s, so the notice stays up long enough to be seen.
  await expect(page.getByTestId("rate-limit-notice")).toContainText("Too many requests");
  await expect(page.getByText("Something went wrong")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Sign in with passkey" })).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText("Something went wrong")).toHaveCount(0);
  await expect(page.getByTestId("rate-limit-notice")).toHaveCount(0);
  expect(meCalls).toBeGreaterThan(LIMITED_CALLS);
});
