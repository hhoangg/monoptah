import base, { restrictedProperties, restrictedSyntax, workerSyntax } from "../eslint.config.js";

// The S-08 browser spec probes that the browser itself refuses these sinks under the CSP and Trusted Types, so it
// must call them. The S-09 sink bans stay on everywhere else (this is a spike, not shipped code).
const probedProperties = new Set(["setHTMLUnsafe"]);

export default [
  ...base,
  {
    files: ["s-08-csp/browser/libs.browser.spec.ts"],
    rules: {
      "no-restricted-syntax": ["error", ...restrictedSyntax, ...workerSyntax],
      "no-restricted-properties": ["error", ...restrictedProperties.filter((entry) => !probedProperties.has(entry.property))],
    },
  },
];
