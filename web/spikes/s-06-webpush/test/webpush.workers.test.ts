import { defineWebPushSuite } from "./suite.ts";

defineWebPushSuite("workerd", () => performance.now());
