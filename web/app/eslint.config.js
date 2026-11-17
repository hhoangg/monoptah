import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import base from "../eslint.config.js";

// Extends the shared base (the XSS bans of security.md section 8 item 1 live there) with the
// React rules. The bans are not relaxed anywhere in this package.
export default [
  ...base,
  reactHooks.configs.flat.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: { globals: { ...globals.browser } },
    // An inline `eslint-disable` must not switch off the security.md 8.1 bans in application code.
    linterOptions: { noInlineConfig: true },
  },
  {
    // public/theme-init.js is a classic script loaded from <head>, not a module.
    files: ["public/**/*.js"],
    languageOptions: { sourceType: "script", globals: { ...globals.browser } },
  },
  {
    files: ["vite.config.ts", "vitest.config.ts", "eslint.config.js", "vite-plugins/**", "scripts/**", "src/**/*.test.{ts,tsx}", "src/test/**"],
    languageOptions: { globals: { ...globals.node } },
  },
];
