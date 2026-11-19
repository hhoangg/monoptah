// Turns off zod's JIT schema compiler (W13-8). Under the app CSP (no 'unsafe-eval') and Trusted Types, zod's probe
// `new Function("")` raises a violation report on the first object schema. zod reads `jitless` when a schema is
// created, so every file of this package that builds schemas imports this module before it imports `zod`.
import { z } from "zod";

z.config({ jitless: true });
