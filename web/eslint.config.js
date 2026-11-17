// Shared ESLint base for every web package (flat config).
//
// A package extends it from its own eslint.config.js:
//
//   import base from "../eslint.config.js";
//   export default [...base, { files: ["src/**/*.tsx"], rules: { /* ... */ } }];
//
// The XSS bans of security.md section 8 item 1 live here so that no package can
// forget them. Do not disable them per file; a real exception needs a reviewed
// entry in security.md first.
import js from "@eslint/js";
import tseslint from "typescript-eslint";

const BAN = "Banned by security.md section 8 item 1 (no raw HTML or code-string sinks).";

/** Selectors for `no-restricted-syntax`. Exported so a package can extend the list. */
export const restrictedSyntax = [
  {
    selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
    message: `dangerouslySetInnerHTML is banned. ${BAN}`,
  },
  {
    selector: "NewExpression[callee.name='Function']",
    message: `new Function is banned. ${BAN}`,
  },
  {
    selector: "CallExpression[callee.name='Function']",
    message: `Function() is banned. ${BAN}`,
  },
  {
    selector: "CallExpression[callee.name='eval']",
    message: `eval is banned. ${BAN}`,
  },
  {
    selector: "CallExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name='eval']",
    message: `eval is banned. ${BAN}`,
  },
];

/** Entries for `no-restricted-properties`. */
export const restrictedProperties = [
  { property: "innerHTML", message: `innerHTML is banned. ${BAN}` },
  { property: "outerHTML", message: `outerHTML is banned. ${BAN}` },
  { property: "insertAdjacentHTML", message: `insertAdjacentHTML is banned. ${BAN}` },
  { object: "document", property: "write", message: `document.write is banned. ${BAN}` },
  { object: "document", property: "writeln", message: `document.writeln is banned. ${BAN}` },
];

export const ignores = ["**/node_modules/**", "**/dist/**", "**/.wrangler/**", "**/coverage/**", "**/*.gen.ts"];

const base = tseslint.config(
  { ignores },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: { ecmaVersion: 2023, sourceType: "module" },
    rules: {
      "no-restricted-syntax": ["error", ...restrictedSyntax],
      "no-restricted-properties": ["error", ...restrictedProperties],
      "no-eval": "error",
      "no-new-func": "error",
      "no-implied-eval": "error",
      "@typescript-eslint/consistent-type-imports": ["error", { fixStyle: "inline-type-imports" }],
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
    },
  },
);

export default base;
