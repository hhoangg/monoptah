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
  },
  {
    // public/theme-init.js is a classic script loaded from <head>, not a module.
    files: ["public/**/*.js"],
    languageOptions: { sourceType: "script", globals: { ...globals.browser } },
  },
  {
    files: ["vite.config.ts", "vitest.config.ts", "eslint.config.js", "src/**/*.test.{ts,tsx}", "src/test/**"],
    languageOptions: { globals: { ...globals.node } },
  },
];
