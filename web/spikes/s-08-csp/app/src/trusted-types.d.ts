// Minimal Trusted Types typings (the DOM lib of this TypeScript version does not include them).
// Needed because the stub imports web/app/src/trusted-types.ts, which uses `window.trustedTypes`.
interface TrustedTypePolicyOptions {
  createHTML?: (input: string) => string;
  createScript?: (input: string) => string;
  createScriptURL?: (input: string) => string;
}
interface TrustedTypePolicyFactory {
  createPolicy(name: string, options?: TrustedTypePolicyOptions): unknown;
}
interface Window {
  trustedTypes?: TrustedTypePolicyFactory;
}
