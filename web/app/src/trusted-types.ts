// Trusted Types `default` policy of security.md 8.2. With the CSP directives
// `require-trusted-types-for 'script'; trusted-types default`, every string that reaches a DOM or
// script sink goes through this policy: HTML and script text are refused, script URLs are allowed
// only for the service worker and same-origin /assets/*.js.

export function installTrustedTypesPolicy(): void {
  window.trustedTypes?.createPolicy("default", {
    createHTML: () => { throw new TypeError("HTML sinks are disabled"); },
    createScript: () => { throw new TypeError("Script sinks are disabled"); },
    createScriptURL: (url) => {
      const u = new URL(url, location.href);
      const ok = u.pathname === "/sw.js" || (u.pathname.startsWith("/assets/") && u.pathname.endsWith(".js"));
      if (u.origin === location.origin && ok) return u.href;
      throw new TypeError("Blocked script URL");
    },
  });
}
