import { defineWebPushSuite } from "./suite.ts";

defineWebPushSuite("node", () => performance.now());
