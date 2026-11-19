import { test as base, expect } from "@playwright/test";
import { Actor } from "./actor.ts";
import { origin } from "./env.ts";
import { testIp } from "./pure.ts";
import { addVirtualAuthenticator } from "./webauthn.ts";

interface Fixtures {
  /** Opens a new browser context on the worker origin with its own virtual authenticator and its own IP. */
  newActor: () => Promise<Actor>;
}

interface WorkerFixtures {
  /**
   * Like `newActor`, but the contexts live until the worker ends, so a serial describe can keep one actor (and its
   * virtual authenticator, which holds the passkey) across its tests. Use it only for that.
   */
  newSharedActor: () => Promise<Actor>;
}

// One IP index for the whole worker process, so no two actors of a run share a per-IP rate limit. 240 and up are
// reserved for the probes in lib/probes.ts.
let nextIp = 10;

export const test = base.extend<Fixtures, WorkerFixtures>({
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
  newSharedActor: [
    async ({ browser }, use) => {
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
    { scope: "worker" },
  ],
});

export { expect };
