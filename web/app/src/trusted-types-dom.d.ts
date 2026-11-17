// Minimal Trusted Types typings (the DOM lib shipped with TypeScript 6 has no `window.trustedTypes`).
export {};

declare global {
  interface TrustedTypePolicyOptions {
    createHTML?: (input: string, ...args: unknown[]) => string;
    createScript?: (input: string, ...args: unknown[]) => string;
    createScriptURL?: (input: string, ...args: unknown[]) => string;
  }

  interface TrustedTypePolicyFactory {
    createPolicy(name: string, options?: TrustedTypePolicyOptions): unknown;
  }

  interface Window {
    trustedTypes?: TrustedTypePolicyFactory;
  }
}
