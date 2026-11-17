# S-08 CSP and Trusted Types with the chosen libraries

Status: DONE for Chromium 159, Firefox 159 and Playwright WebKit 27.2 (a WPE/GTK build, not Safari); real Safari 26 and iOS 18/26 are PENDING-MAC / PENDING-DEVICE.
Evidence: `/tmp/evidence/ws-q/browser/run.log` (96 passed), per-library JSON in `/tmp/evidence/ws-q/browser/s08/` (not committed).
Run: from `web/`, `pnpm --filter @monoptah/spikes run browser`.

## What was built

- Markdown renderer scope: security.md 8 item 1 says the renderer is an in-house AST port (mobile `markdownParser.ts`) that produces React elements, with no third-party markdown library. So no library is tested here; `app/src/Markdown.tsx` is a small stand-in that renders an AST as React elements (literal `<img onerror>` stays text, `javascript:` link rendered as text, https link allowed) and a test shows it causes no violation and no HTML element. The real renderer is covered by WS-W tests.
- `app/`: Vite 8 + React 19 stub. Shiki (JS regex engine, `codeToTokens`, React spans), vaul Drawer (open, close, drag), motion, Sonner, TanStack Router (route change) and Query (fetch of `/api/hello`), Workbox (`workbox-window` registering `/sw.js`, built with `workbox-precaching`). `src/main.tsx` imports `web/app/src/trusted-types-init.ts` by relative path first, so the app's real throwing `default` policy is what runs.
- `server.ts`: Node static server that sends `buildCsp(host)` from `web/worker/src/csp.ts` and `COMPANION_HEADERS` on every response (HTML and `sw.js` included). `serve-all.ts` builds and starts all variants on fixed ports.
- `browser/libs.browser.spec.ts`: a `securitypolicyviolation` listener (init script) plus console CSP/TT messages are collected for every library.

## Acceptance table (roadmap S-08)

| Item | Result | Evidence |
|---|---|---|
| No CSP or TT violation in normal use, or each violation has a patch or replacement | PASS on Chromium, Firefox, Playwright WebKit with two patches (below) | tests "patched build, exact CSP: <lib> has no violation" and "every library in one session" |
| Violations of the unpatched build | 2 `style-src-elem` (Sonner, vaul) at load and 2 more when a drawer opens (Radix scroll lock), same in all 3 engines; Sonner renders unstyled (`position: static`), vaul has `touch-action: auto` | tests "raw unpatched build: <lib> page has the runtime <style> violation" (asserted, plus Sonner `position: static` and vaul `touch-action: auto`) |
| `style-src-attr` scope confirmed minimal | Better than minimal: removing `style-src-attr 'unsafe-inline'` breaks nothing; with the served header asserted to lack the directive, every library is driven with zero violations in 3 engines. Positive control: in the same variant an HTML `style` attribute and a script `setAttribute("style")` are blocked (style-src violation, colour not applied) while with the directive they apply, so the variant is a real test. Proposed: drop the directive (see below) | tests "no style-src-attr variant: <lib> works with zero violations" and "positive control: ..." |
| Service worker registration passes the `default` policy | PASS: `new Workbox("/sw.js").register()` registers scope `/` script `/sw.js` in 3 engines | test "workbox has no violation" (registration facts) |
| TT policy refuses sinks | PASS: `innerHTML`, `setHTMLUnsafe`, `insertAdjacentHTML`, `createContextualFragment`, `DOMParser.parseFromString`, `script.textContent`, `iframe.srcdoc`, `setTimeout(string)` are refused; `script.src` off-origin throws "Blocked script URL"; `/assets/x.js` allowed; `eval` and `new Function` are blocked by CSP (EvalError) | tests "Trusted Types default policy ..." and "code-string sinks ..." (all 11 probed sinks asserted: eval, new Function, setTimeout string, innerHTML, insertAdjacentHTML, createContextualFragment, DOMParser.parseFromString, script.textContent, script.src off-site, iframe.srcdoc, new Worker off-site) |
| ESLint banned API list | see below | |
| iOS 18 fallback documented | see below; decision stands: accept the fallback | |
| Real Safari 26 (macOS), iOS 18 and 26 | PENDING-MAC / PENDING-DEVICE, steps below | |

## The two patches (both proven by tests)

1. **Sonner and vaul runtime `<style>`**. Both packages call `__insertCSS("...")` at import time (`sonner/dist/index.mjs`, `vaul/dist/index.mjs`), which appends a `<style>` that `style-src 'self'` blocks. Sonner exports `sonner/dist/styles.css`, but its JS still injects at runtime (the package has no variant without it); vaul ships `style.css` that its `exports` map hides. Patch: the Vite plugin `staticLibraryCss` in `app/vite.config.ts` removes the `__insertCSS` call and serves the identical string as a CSS module, which Vite emits as a normal same-origin stylesheet. It throws if the call is not found, if either library was never transformed, or if any emitted chunk still contains the libraries' runtime style injection (`createElement(style)` followed by `.type = text/css`, a `generateBundle` check; React DOM's own unused `<style precedence>` path is not matched), so a library update that changes the shape fails the build; the test "build output ..." repeats this on the built files and checks the raw build does contain it. vaul's CSS has one invalid rule (`[data-vaul-handle-hitarea]:{...}`) that browsers drop at parse time and the minifier rejects; the plugin drops it too (same runtime behaviour).
2. **Radix scroll lock inside vaul**. `react-remove-scroll-bar` (via `react-style-singleton`) builds a `<style>` with the measured scrollbar gap each time a modal drawer opens. Patch: a Vite alias of `react-style-singleton` to `app/src/patches/style-singleton.ts`, which adopts a constructed stylesheet (`new CSSStyleSheet()` + `document.adoptedStyleSheets`). Constructed stylesheets are CSSOM, not subject to the inline-style check. Works in all 3 engines. Alternative if the alias is unwanted: `Drawer.Root modal={false}` (loses focus trap and scroll lock). Needs adopted stylesheets (Safari 16.4+).

Both patches are build config, not source changes to the libraries. The real app needs the same plugin and alias in its Vite config (owner: web/app).

## Findings that change the docs

- **`style-src-attr 'unsafe-inline'` is not needed.** React `style` props, motion and vaul set styles through CSSOM (`element.style.x = ...`), which CSP does not block. The variant without the directive (port 4181/4185) ran all libraries clean. What the directive would still cover: `setAttribute("style", ...)` and parsed `style="..."` HTML, neither of which the app uses (no HTML sinks). Recommend removing it from `buildCsp` and security.md 8.1 once Safari 26 confirms (PENDING-MAC step 3), which also deletes the stated reason in 8.1. If kept, state it is defense-in-depth slack, not a requirement.
- **`upgrade-insecure-requests` breaks `http://localhost` in WebKit**: it upgrades subresources to `https://localhost:PORT` and the page loads unstyled and without script (TLS handshake error). Chromium and Firefox exempt localhost. Production is https so the real app is unaffected; the same build over https with the exact CSP (self-signed cert, port 4186) passes in all 3 engines (test "exact CSP over https"). Consequence for dev and tests: serve WebKit over https or strip that one directive; the spikes do the latter on 4183-4185.
- **Firefox blocks port 4190** ("Blocked Page"); unrelated to the app, noted because the first S-03 server used it.
- **Playwright's `page.evaluate` does not enforce the CSP `eval` block** in Chromium (it ran `eval` and `new Function` successfully). Tests of code-string sinks must use a served page script; `server.ts` serves `/__probe.html` for that.

## ESLint banned API list

Enforced today by `web/eslint.config.js`: `dangerouslySetInnerHTML` (JSX), `new Function`, `Function()`, `eval`, `window|globalThis|self.eval`; properties `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `document.writeln`; rules `no-eval`, `no-new-func`, `no-implied-eval`.

Proposed additions (all are Trusted Types sinks that the spike showed are refused at runtime, so lint gives an earlier error): `setHTMLUnsafe`, `Document.parseHTMLUnsafe`, `createContextualFragment`, `DOMParser.parseFromString` (HTML), `iframe.srcdoc` and `srcdoc` JSX attribute, `setAttribute` with a literal `"style"`, `"srcdoc"` or an `on*` name, `new Worker`/`importScripts`/`SharedWorker` outside an allowlisted module, `document.createElement("script")` and `script.src/text/textContent`, and `setTimeout`/`setInterval` with a string literal first argument (covered by `no-implied-eval`). Needed from: web/eslint.config.js owner.

## iOS 18 fallback (no Trusted Types before Safari 26)

- caniuse (https://caniuse.com/trusted-types, read 2026-11-17): Safari desktop supports from 26.0, not 3.1 to 18.7; iOS Safari supports from 26.0, not 3.2 to 18.7.
- On those versions the CSP `require-trusted-types-for` and `trusted-types` directives are ignored, so only the rest of the policy applies: `script-src 'self'` (no inline, no eval), `style-src 'self'`, `connect-src` and the others. This spike confirms with Playwright WebKit 27.2 that TT is enforced when present (it is a TT-capable build, so it cannot show the iOS 18 behaviour; PENDING-DEVICE below).
- Decision (finalize in security.md 8.2): accept the fallback. Protection on iOS 18 is the strict CSP, the AST markdown renderer, shiki as tokens, and the ESLint bans. The stub shows there is no HTML sink in the normal use of any chosen library, so TT adds a safety net, not a dependency.

## PENDING-MAC (Safari 26 on macOS)

1. On the Mac: `cd web && pnpm --filter @monoptah/spikes exec node s-08-csp/serve-all.ts` (builds and serves; Ctrl-C to stop).
2. In Safari 26 open `https://localhost:4186/` (self-signed; accept the warning once, or `http://localhost:4184/`). Open Web Inspector, Console and Network, enable "Preserve log".
3. Exercise: toast button, drawer open/close/drag, route link, "register sw". Record any console line containing "Refused" or "TrustedHTML". Then open `http://localhost:4185/` (no `style-src-attr`) and repeat; expected: identical, no errors. Record Safari version.
4. Open `https://localhost:4186/__probe.html` and read `window.__probe` in the console (all sinks must say "requires a Trusted...").

## PENDING-DEVICE (iOS 18 and iOS 26)

1. Mac and phone on the same Wi-Fi. Find the Mac LAN IP (`ipconfig getifaddr en0`). The cert must name that IP: run `openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=lan -addext subjectAltName=IP:<LAN-IP> -keyout /tmp/k.pem -out /tmp/c.pem`, or tunnel with `cloudflared tunnel --url http://localhost:4180` (or `ngrok http 4180`), which gives a real https URL with the exact CSP (the tunnel terminates https, so `upgrade-insecure-requests` is harmless).
2. Open the URL in Safari on iOS 18 (no TT) and iOS 26 (TT). Use the page buttons as in the Mac steps. On iOS 18 `typeof window.trustedTypes` must be `undefined` and `/__probe.html` sinks say "allowed" (expected: the fallback), while `eval` and inline style injection stay blocked by CSP.
3. Record OS version, Safari version, console errors, toast/drawer behaviour, SW registered, in the table below.

| Device | OS | Result | Notes |
|---|---|---|---|
| iPhone Safari tab | 18.x | PENDING-DEVICE | |
| iPhone Safari tab | 26.x | PENDING-DEVICE | |
| Mac Safari | 26.x | PENDING-MAC | |

## Proposed doc and config changes (owners must apply; spikes do not edit them)

- security.md 8.1: remove `style-src-attr 'unsafe-inline'` and its justification sentence (after PENDING-MAC step 3); note that the CSP dev server needs https or no `upgrade-insecure-requests` in WebKit.
- `web/worker/src/csp.ts` `buildCsp`: drop the `style-src-attr` line (same condition), keep a test that the policy contains no `unsafe-inline`.
- security.md 8.2: replace "Library compatibility ... is a spike item" with the result above (React 19, TanStack Router/Query, motion, Workbox, shiki tokens need nothing; Sonner and vaul need the two build patches), and finalize "accept the fallback" for iOS 18.
- web/app Vite config: add the `staticLibraryCss` plugin and the `react-style-singleton` alias (copy from `s-08-csp/app/vite.config.ts` and `app/src/patches/style-singleton.ts`), with a build test that fails if a built asset still contains `createElement("style")`.
- web/eslint.config.js: the proposed sink list above.
- Dependency note: pin Sonner and vaul versions the plugin was verified against (sonner 2.0.8, vaul 1.1.2); re-run this spike on upgrades.
