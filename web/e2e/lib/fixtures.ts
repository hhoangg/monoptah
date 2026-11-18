import { test as base, expect } from "@playwright/test";
import { Actor } from "./actor.ts";
import { origin } from "./env.ts";
import { testIp } from "./pure.ts";
import { addVirtualAuthenticator } from "./webauthn.ts";

interface Fixtures {
  /** Opens a new browser context on the worker origin with its own virtual authenticator and its own IP. */
  newActor: () => Promise<Actor>;
}

// One IP index for the whole worker process, so no two actors of a run share a per-IP rate limit. 240 and up are
// reserved for the probes in lib/probes.ts.
let nextIp = 10;

export const test = base.extend<Fixtures>({
  newActor: async ({ browser }, use) => {
    const opened: Actor[] = [];
    await use(async () => {
      const context = await browser.newContext({ baseURL: origin() });
      const page = await context.newPage();
      await page.goto("/");
      const authenticator = await addVirtualAuthenticator(context, page);
      const actor = new Actor(context, page, authenticator, origin(), testIp(nextIp++));
      opened.push(actor);
      return actor;
    });
    for (const actor of opened) {
      await actor.authenticator.dispose();
      await actor.context.close();
    }
  },
});

export { expect };
