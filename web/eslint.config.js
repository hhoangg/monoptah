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
    selector: "NewExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name='Function']",
    message: `new globalThis.Function is banned. ${BAN}`,
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

const S09 = "Banned by security.md section 8 (S-09): no HTML parsing sinks, script creation or extra workers.";

/** S-09 bans on parsing and script sinks. Part of every package's `no-restricted-syntax`. */
export const sinkSyntax = [
  { selector: "NewExpression[callee.name='DOMParser']", message: `DOMParser is banned. ${S09}` },
  { selector: "NewExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name='DOMParser']", message: `DOMParser is banned. ${S09}` },
  { selector: "JSXAttribute[name.name=/^srcdoc$/i]", message: `The srcdoc attribute is banned. ${S09}` },
  {
    selector: "CallExpression[callee.property.name='setAttribute'][arguments.0.value=/^(style|srcdoc|on.*)$/i]",
    message: `setAttribute with "style", "srcdoc" or an on* name is banned. ${S09}`,
  },
  {
    // The namespace is argument 0; the attribute name is argument 1.
    selector: "CallExpression[callee.property.name='setAttributeNS'][arguments.1.value=/^(style|srcdoc|on.*)$/i]",
    message: `setAttributeNS with "style", "srcdoc" or an on* name is banned. ${S09}`,
  },
  {
    selector: "CallExpression[callee.property.name='createElement'][arguments.0.value=/^script$/i]",
    message: `Creating a script element is banned. ${S09}`,
  },
  {
    selector: "CallExpression[callee.property.name='createElementNS'][arguments.1.value=/^script$/i]",
    message: `Creating a script element is banned. ${S09}`,
  },
  { selector: "JSXOpeningElement[name.name='script']", message: `A script element is banned. ${S09}` },
  {
    selector: "AssignmentExpression[left.object.name=/^script/i][left.property.name=/^(src|text|textContent|innerText)$/]",
    message: `Setting src, text or textContent of a script is banned. ${S09}`,
  },
  { selector: "CallExpression[callee.name='importScripts']", message: `importScripts is banned. ${S09}` },
];

/** `new Worker` and `new SharedWorker`: banned except in the modules a package allowlists (see its eslint config). */
export const workerSyntax = [
  { selector: "NewExpression[callee.object.name=/^(window|globalThis|self)$/][callee.property.name=/^(Worker|SharedWorker)$/]", message: `new window.Worker is banned outside the allowlisted worker modules. ${S09}` },
  { selector: "NewExpression[callee.name=/^(Worker|SharedWorker)$/]", message: `new Worker and new SharedWorker are banned outside the allowlisted worker modules. ${S09}` },
];

/** Entries for `no-restricted-properties`. */
export const restrictedProperties = [
  { property: "innerHTML", message: `innerHTML is banned. ${BAN}` },
  { property: "outerHTML", message: `outerHTML is banned. ${BAN}` },
  { property: "insertAdjacentHTML", message: `insertAdjacentHTML is banned. ${BAN}` },
  { object: "document", property: "write", message: `document.write is banned. ${BAN}` },
  { object: "document", property: "writeln", message: `document.writeln is banned. ${BAN}` },
  { property: "setHTMLUnsafe", message: `setHTMLUnsafe is banned. ${S09}` },
  { property: "parseHTMLUnsafe", message: `Document.parseHTMLUnsafe is banned. ${S09}` },
  { property: "createContextualFragment", message: `createContextualFragment is banned. ${S09}` },
  { property: "parseFromString", message: `DOMParser.parseFromString is banned. ${S09}` },
  { property: "srcdoc", message: `iframe srcdoc is banned. ${S09}` },
  { property: "srcDoc", message: `iframe srcDoc is banned. ${S09}` },
];

export const ignores = ["**/node_modules/**", "**/dist/**", "**/.wrangler/**", "**/coverage/**", "**/*.gen.ts"];

const base = tseslint.config(
  { ignores },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: { ecmaVersion: 2023, sourceType: "module" },
    rules: {
      "no-restricted-syntax": ["error", ...restrictedSyntax, ...sinkSyntax, ...workerSyntax],
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
