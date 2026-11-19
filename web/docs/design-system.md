# Monoptah Web - Design System

The web app is visually identical to the mobile app. This file is the contract: tokens, Tailwind v4 theme, shadcn mapping, motion, layouts, accessibility and a checklist for new features. Sources are paths relative to the repo root (mobile app under `mobile/`).

Ground rules inherited from mobile:

- No box shadows anywhere (`mobile/src/features/sessions/LiveSessions.tsx:736`, "No shadow"). Depth = surface color steps + 1px strokes + glass blur + dimming scrims.
- System fonts only, weights 400/500/600/700.
- Units: RN points = CSS px. Use `rem` for type and spacing so browser text scaling works (1rem = 16px, never clamp zoom).
- Hairline = `1px` (the ChoiceMenu divider uses `0.5px`).

## 1. Color tokens

The palette has 21 keys (`mobile/src/ui/theme.ts:11-46`). Values verified against the source.

| Token | Role |
|---|---|
| background / drawer | page and sidebar background |
| sheet | sheets, dialogs, popovers (one step above background) |
| surface | cards, input wells |
| surfaceRaised | menu cards inside sheets, chips |
| stroke | 1px borders and dividers |
| selection | selected / active row |
| content / muted / faint | primary / secondary / tertiary text and icons |
| accent | links, toggles, unread dot, spinner, selected checks |
| additions / deletions | success green / danger red (diff, errors, destructive) |
| caution | amber: warnings, full access, "needs you" |
| fileRef | file names in transcript |
| inverse / inverseSurface | primary button pair (bg inverseSurface, text inverse) |
| fill | round buttons, pills, chips over glass |
| thumb | segmented switch thumb |
| mark | logo tint |
| rail | transcript fold rails |
| bubble | user message bubble |
| listCard / listDivider | file list card and row dividers |

### 1.1 Classic ("mono", default) - `theme.ts:11-77`

| Token | Dark | Light |
|---|---|---|
| background, drawer | `#131313` | `#f7f7f7` |
| sheet | `#181818` | `#ffffff` |
| surface | `#202020` | `#ededed` |
| surfaceRaised | `#2a2a2a` | `#e3e3e3` |
| stroke | `rgba(235,235,235,.08)` | `rgba(46,46,46,.10)` |
| selection | `rgba(235,235,235,.10)` | `rgba(46,46,46,.06)` |
| content | `#ebebeb` | `#2e2e2e` |
| muted | `#8c8c8c` | `#737373` |
| faint | `#5c5c5c` | `#a6a6a6` |
| accent | `#4c9ef5` | `#1f7ae0` |
| additions | `#6ee7b7` | `#047857` |
| deletions | `#f87171` | `#be123c` |
| caution | `#fbbf24` | `#b45309` |
| fileRef | `#c4b5fd` | `#6d28d9` |
| inverse | `#0c0c0c` | `#fafafa` |
| inverseSurface | `#f2f2f2` | `#262626` |
| fill | `rgba(255,255,255,.08)` | `rgba(46,46,46,.09)` |
| thumb | `rgba(255,255,255,.14)` | `rgba(46,46,46,.10)` |
| mark | `#ffffff` | `#2e2e2e` |
| rail | `#323232` | `#dadada` |
| bubble | `#292929` | `#e9e9e9` |
| listCard | `#1e1e1e` | `#ffffff` |
| listDivider | `#2f2f2f` | `#e6e6e6` |

### 1.2 Tinted themes - `theme.ts:82-165`

Five themes tint Classic's surfaces and set their own accent. Keys not listed inherit Classic: stroke, selection, content, muted, faint, additions, deletions, caution, fileRef, inverse, inverseSurface, fill, thumb, mark (light also inherits sheet and listCard `#ffffff`). Values below are the exact hex output of the app's `hsl()` helper.

| Theme (data-theme, label, hue) | Scheme | background=drawer | sheet | surface | surfaceRaised | rail | bubble | listCard | listDivider | accent |
|---|---|---|---|---|---|---|---|---|---|---|
| ocean "Cobalt" 212 | dark | `#0e1216` | `#12171c` | `#1b2025` | `#242a30` | `#2b3036` | `#22282f` | `#171c21` | `#272d34` | `#5aa9f7` |
| | light | `#f3f6f9` | `#ffffff` | `#e8ecf0` | `#dce1e7` | `#d3d8df` | `#e3e8ed` | `#ffffff` | `#e0e4e9` | `#1f6fc4` |
| grove "Jade" 150 | dark | `#0e1612` | `#121c17` | `#1b2520` | `#24302a` | `#2b3630` | `#222f29` | `#17211c` | `#27342e` | `#4cc38a` |
| | light | `#f3f9f6` | `#ffffff` | `#e8f0ec` | `#dce7e2` | `#d3dfd9` | `#e3ede8` | `#ffffff` | `#e0e9e4` | `#13804f` |
| ember "Copper" 22 | dark | `#16110e` | `#1c1612` | `#251f1b` | `#302924` | `#362f2b` | `#2f2722` | `#211b17` | `#342c27` | `#f0884a` |
| | light | `#f9f5f3` | `#ffffff` | `#f0ebe8` | `#e7e0dc` | `#dfd7d3` | `#ede7e3` | `#ffffff` | `#e9e3e0` | `#c2531a` |
| iris "Violet" 262 | dark | `#110e16` | `#16121c` | `#1f1b25` | `#292430` | `#2f2b36` | `#27222f` | `#1b1721` | `#2c2734` | `#9d87f5` |
| | light | `#f5f3f9` | `#ffffff` | `#ebe8f0` | `#e0dce7` | `#d7d3df` | `#e7e3ed` | `#ffffff` | `#e3e0e9` | `#6a4fd6` |
| rose "Blush" 338 | dark | `#160e11` | `#1c1216` | `#251b1f` | `#302429` | `#362b2f` | `#2f2227` | `#21171b` | `#34272c` | `#ec6fa3` |
| | light | `#f9f3f5` | `#ffffff` | `#f0e8eb` | `#e7dce0` | `#dfd3d7` | `#ede3e7` | `#ffffff` | `#e9e0e3` | `#c42a6c` |

Formulas (to regenerate): dark background `hsl(H 22% 7%)`, sheet `hsl(H 20% 9%)`, surface `hsl(H 16% 12.5%)`, raised `hsl(H 14% 16.5%)`, rail `hsl(H 12% 19%)`, bubble `hsl(H 16% 16%)`, listCard `hsl(H 18% 11%)`, listDivider `hsl(H 14% 18%)`. Light background `hsl(H 30% 96.5%)`, surface `hsl(H 22% 92.5%)`, raised `hsl(H 18% 88.5%)`, rail `hsl(H 16% 85%)`, bubble `hsl(H 22% 91%)`, listDivider `hsl(H 16% 89.5%)`.

Default theme `mono`, default accent `theme` (`theme.ts:171-178, 222-229`).

### 1.3 Accents - `theme.ts:208-216`

| data-accent | Dark | Light |
|---|---|---|
| theme (default) | theme's own | theme's own |
| blue | `#4c9ef5` | `#1f7ae0` |
| teal | `#3cc4c4` | `#0f7f86` |
| green | `#4cc38a` | `#13804f` |
| orange | `#f0884a` | `#c2531a` |
| red | `#f06a6a` | `#c9302c` |
| pink | `#ec6fa3` | `#c42a6c` |
| violet | `#9d87f5` | `#6a4fd6` |

An accent replaces only `accent` (`theme.ts:250-264`). A subtree can override it (Mono chat color, `theme.ts:294`): set `--accent-color` inline on that element.

### 1.4 Content alpha ladder

The transcript derives tones from `content` with alpha (`withAlpha`, `theme.ts:334`). Define `--content-a04 ... --content-a85` (plus `--content-a72`, used only by the section 9 override) as `color-mix(in srgb, var(--content) N%, transparent)`:

| Alpha | Used for (`TranscriptTurn.tsx`, `Markdown.tsx`) |
|---|---|
| 0.04 | error notice bg |
| 0.05 | code block bg, mini pill, pressed row |
| 0.07 | inline code, signature pill, day pill |
| 0.08 | active footer button |
| 0.25 / 0.35 / 0.4 | separator dots / footer time / user time |
| 0.45 / 0.5 / 0.55 | steps, day text / fold labels, notices / working status |
| 0.6 / 0.65 | "show more" / muted prose |
| 0.75-0.8 / 0.85 | names / code text |

Accessibility override in section 9 (light mode needs higher alphas).

### 1.5 Fixed colors outside the palette

| Use | Value | Source |
|---|---|---|
| Diff add row / gutter | `rgba(16,185,129,.15)` / `.25` | `src/features/changes/diffTheme.ts:19` |
| Diff del row / gutter | `rgba(244,63,94,.15)` / `.25` | `diffTheme.ts:20` |
| Diff add text dark / light | `#6ee7b7` / `#047857` | `diffTheme.ts:25,30` |
| Diff del text dark / light | `#fda4af` / `#be123c` | `diffTheme.ts:26,31` |
| Git status M/A/D/U dark | `#fbbf24 #6ee7b7 #fda4af #38bdf8` | `diffTheme.ts:24` |
| Git status M/A/D/U light | `#b45309 #047857 #be123c #0284c7` | `diffTheme.ts:29` |
| Syntax | github-dark / github-light palettes (use shiki themes `github-dark`, `github-light`) | `src/features/files/CodeView.tsx:34-46` |
| Chart hues [dark, light] | pos `#4fd57f/#0a924b`, neg `#fc5855/#d01d21`, warn `#fea247/#d96700`, `#b77df8/#863ad2`, `#fa78b2/#d12c80`, `#3faff3/#0077d7` | `src/features/visuals/charts.tsx:15-24` |
| Status: needs input / done / failed | `#fbbf24` / `rgba(52,211,153,.9)` / deletions | `LaunchedSessionCard.tsx:48-52` |
| Status: working / draft / stopped | content at .55 / .45 / .45 | `LaunchedSessionCard.tsx:47-51` |
| Unread badge | bg `#ff3b30`, text `#fff` 12/600, min 20x20, r10 | `SessionToolbar.tsx:375-387` |
| Modal scrim | `rgba(0,0,0,.4)` | `ModalSheet.tsx:97`, `ChoiceMenu.tsx:144` |
| Drawer scrim | `#000` at .5 dark, 0 light | `ScenePager.tsx:60,594` |
| Project colors | `hsl(210 8% 58%)`, `hsl(211 92% 62%)`, `hsl(12 80% 58%)`, `hsl(45 90% 55%)`, `hsl(142 55% 50%)`, `hsl(330 70% 62%)`, `hsl(280 55% 62%)`, `hsl(175 55% 48%)`, `hsl(25 85% 58%)` | `src/features/workspace/projectColors.ts:3-13` |
| Desktop / Mono swatches | `#459cf7 #ea603e #f4bf25 #39c363 #e45a9f #b168d4 #37beb4 #ef8c39 #8b63e9` | `src/features/workspace/monoLook.ts:3` |

Desktop colors in the web desktop manager reuse the swatch list.

## 2. Typography

No bundled fonts (no `expo-font`). Mobile uses the platform system font and `Menlo` for code (`diffTheme.ts:6`, `Markdown.tsx:24`). Web decision: no webfont download.

```
--font-sans: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Roboto, system-ui, sans-serif;
--font-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
```

No letter-spacing anywhere (tracking normal). Use `font-variant-numeric: tabular-nums` on times and counts (`SessionCard.tsx:114`, `WorkspaceDrawer.tsx:571`, `LiveSessions.tsx:772`, `FinishedSessions.tsx:394`).

| Token | Size / line-height | Weight | Used for | Source |
|---|---|---|---|---|
| text-display | 34 / 40 | 600 | connect / sign-in title | `ConnectScreen.tsx:357` |
| text-headline | 22 / 28 | 600 | empty-session headline | `EmptySession.tsx:170` |
| text-title | 20 / 1.3 | 600 | sheet and menu titles | `ChoiceMenu.tsx:168` |
| text-h | 18 / 26 | 600 | markdown H1/H2 | `Markdown.tsx:351` |
| text-row | 17 / 22 | 400 (600 drawer row, 700 header session name) | rows, composer input | `SettingsUI.tsx:148`, `Composer.tsx:739` |
| text-body | 16 / 24 | 400 (600 buttons, card titles) | prose, fold labels | `Markdown.tsx:339`, `TranscriptTurn.tsx:39` |
| text-bubble | 16 / 23 | 400 | user bubble | `TranscriptTurn.tsx:562` |
| text-label | 15 / 20 | 400-500 | secondary labels, model chip, question prompt, inline code (mono) | `appearance.tsx:378`, `Composer.tsx:787` |
| text-small | 14 / 20 | 400-600 | signature, notices, tables, dialog buttons | `TranscriptTurn.tsx:686` |
| text-meta | 13 / 18-19 | 400-600 | notes, meta lines, code text (mono 13/19), section labels | `appearance.tsx:398`, `Markdown.tsx:395` |
| text-caption | 12 / 16-17 | 400-600 | day pill, timestamps, badges, hints | `TranscriptTurn.tsx:529` |
| text-micro | 11 / 14 | 400 | reaction chip | `TranscriptTurn.tsx:600` |
| text-emoji | 56 / 64 | n/a | emoji-only message | `TranscriptTurn.tsx:583` |

Weights: 400 default, 500 labels and chips, 600 titles, buttons, `**strong**`, 700 header session name only. Mobile honors OS font scale, so web uses rem and never sets `maximum-scale`.

## 3. Spacing, radii, borders

### 3.1 Spacing

Base grid 2/4 px, no named scale. Expose Tailwind's default `--spacing: 0.25rem` and use these recurring values:

| Value | Where |
|---|---|
| 20 | page and transcript gutter, collapsed composer margin (`SettingsUI.tsx:125`, `TranscriptTurn.tsx:37`, `Composer.tsx:63`) |
| 12 | expanded composer margin, block gap, card gap |
| 28 | connect screen, mini list (`ConnectScreen.tsx:354`) |
| 6 | most common in-row gap; drawer inset (`WorkspaceDrawer.tsx:51`) |
| 14, 16, 18, 24, 40, 56 | drawer row gap, sheet padding, intro stack, section top, page bottom beyond safe area, bubble left offset |

Minimum row heights: session row 44, settings row 52, menu row 56, drawer Mono row 64, project row 52, header 44, composer button 36, circle button 44, mini pill 52, finished card 48. Card paddings: settings card 10h/6v, question card 16h/12v, usage card 14, error notice 14. Bottom safe area: page padding = inset + 40, sheet = inset + 16.

### 3.2 Radii - `theme.ts:266-270`

| Token | px | Use |
|---|---|---|
| `--radius-pill` | 999 | pills, round buttons, avatars |
| `--radius-card` | 22 | cards |
| `--radius-row` | 12 | rows, inputs, options |
| `--radius-sheet` | 32 | sheet top corners, primary big button (`ModalSheet.tsx:105`, `ConnectScreen.tsx:362`) |
| `--radius-menu-card` | 26 | menu cards in sheets (`ChoiceMenu.tsx:172`) |
| `--radius-dialog` | 20 | dialogs, rename (`SessionToolbar.tsx:388`) |
| 21 / 14 | | bubble round / square (`TranscriptTurn.tsx:557`) |
| 18, 16, 10, 8, 6, 5 | | usage card, error notice, code block / thumb / track, table / key chip, footer button, checkbox |

Mobile uses iOS continuous corners. Web: plain `border-radius`, and `corner-shape: squircle` as progressive enhancement (`@supports (corner-shape: squircle)`).

### 3.3 Borders

- Divider: 1px `stroke`, inset 40 in cards with icons, 52 in menus with icons, 16 flush (`SettingsUI.tsx:155`).
- Outlined control: 1px `stroke`. Active option: border `muted` + bg `selection` (`QuestionCard.tsx:391`).
- Radio / checkbox: 18x18, 1.5px border `faint`, radio r9, checkbox r5, active fill + border `content`.
- Accent ring: 38px ring, 2px border (transparent, accent when selected) around a 28px dot (`appearance.tsx:468`).
- Rail: 1px `rail` spine + 8x8 quarter-circle branch (`TranscriptTurn.tsx:629`). Blockquote: 2px left, content at .2, padding-left 12.
- Shadows: none.

## 4. Glass and blur

### 4.1 Glass surface (`mobile/src/ui/LiquidView.tsx`)

Mobile on iOS 26 uses native Liquid Glass; elsewhere (the fallback the web reproduces) it is `BlurView intensity 30`, a wash of `surface` at .92 alpha, and a gradient hairline border (`LiquidView.tsx:36-41,263,266`).

```css
.glass {
  position: relative;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--surface) 92%, transparent);
  -webkit-backdrop-filter: blur(16px) saturate(1.2);
  backdrop-filter: blur(16px) saturate(1.2);
}
.glass::before { /* gradient hairline */
  content: ""; position: absolute; inset: 0; padding: 1px; border-radius: inherit; pointer-events: none;
  background: linear-gradient(135deg, rgba(255,255,255,.10), transparent 35%, transparent 65%, rgba(255,255,255,.09));
  -webkit-mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0);
  mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0);
}
@supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
  .glass { background: var(--sheet); } /* opaque fallback, keep the hairline */
}
@media (prefers-reduced-transparency: reduce) {
  .glass { background: var(--sheet); backdrop-filter: none; -webkit-backdrop-filter: none; }
}
```

Blur radius 16px is an approximation of expo-blur intensity 30 (to tune visually). Custom `tintColor` replaces the wash color via `--glass-tint`.

Press behaviour: scale to 1.05 with the press spring (section 6), light haptic, hit slop 4px (use `::after` inset -4px). Pop-in (glass appearing): from translateY 40, scale .85, opacity never below .03 (`LiquidView.tsx:60-93`).

### 4.2 Gradual blur (`mobile/src/ui/GradualBlur.tsx`)

Three stacked layers (`steps=3`), each `backdrop-filter: blur` at stepped radii 4/8/12px (to tune), wash `color-mix(background 50%, transparent)`, and layer i masked by `linear-gradient(to bottom, #000 0, rgba(0,0,0,.8) cutoff*50%, transparent cutoff*100%)` with `cutoff = 1 - i/3`; bottom edge mirrored. `pointer-events: none`. Heights: header fade 36, composer fade 16, pages without composer 40 (`SessionScreen.tsx:71-75`).

### 4.3 Scrims

0.4 black for sheets and dialogs, 0.5 black for the mobile drawer in dark only.

## 5. CSS: tokens, theme selection, persistence

### 5.1 Selection and persistence

| Preference | Values | Default | Stored in | Attribute on `<html>` |
|---|---|---|---|---|
| appearance | `system` / `light` / `dark` | system | `localStorage["monoptah.appearance"]` | `data-scheme="light|dark"` (resolved) |
| theme | mono, ocean, grove, ember, iris, rose | mono | `monoptah.theme` | `data-theme` |
| accent | theme, blue, teal, green, orange, red, pink, violet | theme | `monoptah.accent` | `data-accent` |

Rules (mobile equivalent `mobile/src/features/settings/appearance.ts:6-51`):

1. A blocking script in `index.html` `<head>` reads the three keys and sets the attributes before first paint, resolving `system` through `matchMedia("(prefers-color-scheme: dark)")`. It also sets `<meta name="theme-color">`. CSP has `script-src 'self'`, so this is a same-origin blocking `/theme-init.js` loaded with a plain `<script src="/theme-init.js">` in `<head>` (no `async`, no `defer`); no inline script and no `unsafe-inline`.
2. A Zustand `appearance` store owns the values, writes to localStorage and the attributes, and listens to `storage` events (cross-tab) and the media query (system mode).
3. Add `<meta name="color-scheme" content="light dark">`; set `color-scheme` in CSS per scheme so form controls and scrollbars match.
4. Preferences are device-local (not synced through the server, which stores metadata only).
5. PWA manifest `theme_color` / `background_color` = `#131313` (dark default); update the meta tag at runtime to the active `background`.

### 5.2 CSS variables

Values are shown for Classic plus one tinted theme; the others follow the tables in 1.2 (generate them from the hue formulas with a build script or copy the table).

```css
/* tokens.css - raw palette, scheme + theme */
:root,
:root[data-scheme="light"] {
  color-scheme: light;
  --background: #f7f7f7;  --drawer: #f7f7f7;  --sheet: #ffffff;
  --surface: #ededed;     --surface-raised: #e3e3e3;
  --stroke: rgba(46,46,46,.10);  --selection: rgba(46,46,46,.06);
  --content: #2e2e2e;  --muted: #737373;  --faint: #a6a6a6;
  --accent-color: #1f7ae0;
  --additions: #047857;  --deletions: #be123c;  --caution: #b45309;  --file-ref: #6d28d9;
  --inverse: #fafafa;  --inverse-surface: #262626;
  --fill: rgba(46,46,46,.09);  --thumb: rgba(46,46,46,.10);  --mark: #2e2e2e;
  --rail: #dadada;  --bubble: #e9e9e9;  --list-card: #ffffff;  --list-divider: #e6e6e6;
  --scrim: rgba(0,0,0,.4);
}
:root[data-scheme="dark"] {
  color-scheme: dark;
  --background: #131313;  --drawer: #131313;  --sheet: #181818;
  --surface: #202020;     --surface-raised: #2a2a2a;
  --stroke: rgba(235,235,235,.08);  --selection: rgba(235,235,235,.10);
  --content: #ebebeb;  --muted: #8c8c8c;  --faint: #5c5c5c;
  --accent-color: #4c9ef5;
  --additions: #6ee7b7;  --deletions: #f87171;  --caution: #fbbf24;  --file-ref: #c4b5fd;
  --inverse: #0c0c0c;  --inverse-surface: #f2f2f2;
  --fill: rgba(255,255,255,.08);  --thumb: rgba(255,255,255,.14);  --mark: #ffffff;
  --rail: #323232;  --bubble: #292929;  --list-card: #1e1e1e;  --list-divider: #2f2f2f;
}

/* Tinted themes: override only what the theme tints (see 1.2). Pattern: */
:root[data-theme="ocean"][data-scheme="dark"] {
  --background: #0e1216; --drawer: #0e1216; --sheet: #12171c; --surface: #1b2025;
  --surface-raised: #242a30; --rail: #2b3036; --bubble: #22282f;
  --list-card: #171c21; --list-divider: #272d34; --accent-color: #5aa9f7;
}
:root[data-theme="ocean"][data-scheme="light"] {
  --background: #f3f6f9; --drawer: #f3f6f9; --surface: #e8ecf0; --surface-raised: #dce1e7;
  --rail: #d3d8df; --bubble: #e3e8ed; --list-divider: #e0e4e9; --accent-color: #1f6fc4;
}
/* repeat for grove, ember, iris, rose with the 1.2 values; "mono" needs no block */

/* Accent over a theme: wins by source order and by specificity */
:root[data-accent="blue"][data-scheme="dark"]   { --accent-color: #4c9ef5; }
:root[data-accent="blue"][data-scheme="light"]  { --accent-color: #1f7ae0; }
:root[data-accent="teal"][data-scheme="dark"]   { --accent-color: #3cc4c4; }
:root[data-accent="teal"][data-scheme="light"]  { --accent-color: #0f7f86; }
:root[data-accent="green"][data-scheme="dark"]  { --accent-color: #4cc38a; }
:root[data-accent="green"][data-scheme="light"] { --accent-color: #13804f; }
:root[data-accent="orange"][data-scheme="dark"] { --accent-color: #f0884a; }
:root[data-accent="orange"][data-scheme="light"]{ --accent-color: #c2531a; }
:root[data-accent="red"][data-scheme="dark"]    { --accent-color: #f06a6a; }
:root[data-accent="red"][data-scheme="light"]   { --accent-color: #c9302c; }
:root[data-accent="pink"][data-scheme="dark"]   { --accent-color: #ec6fa3; }
:root[data-accent="pink"][data-scheme="light"]  { --accent-color: #c42a6c; }
:root[data-accent="violet"][data-scheme="dark"] { --accent-color: #9d87f5; }
:root[data-accent="violet"][data-scheme="light"]{ --accent-color: #6a4fd6; }

/* Derived tokens (declared once) */
:root {
  --accent-text: var(--accent-color);                           /* light override in section 9 */
  --content-a04: color-mix(in srgb, var(--content) 4%, transparent);
  --content-a05: color-mix(in srgb, var(--content) 5%, transparent);
  --content-a07: color-mix(in srgb, var(--content) 7%, transparent);
  --content-a08: color-mix(in srgb, var(--content) 8%, transparent);
  --content-a35: color-mix(in srgb, var(--content) 35%, transparent);
  --content-a40: color-mix(in srgb, var(--content) 40%, transparent);
  --content-a45: color-mix(in srgb, var(--content) 45%, transparent);
  --content-a50: color-mix(in srgb, var(--content) 50%, transparent);
  --content-a65: color-mix(in srgb, var(--content) 65%, transparent);
  --content-a72: color-mix(in srgb, var(--content) 72%, transparent);
  --content-a85: color-mix(in srgb, var(--content) 85%, transparent);

  /* shadcn contract (note: --accent is shadcn's hover surface, NOT the brand accent) */
  --foreground: var(--content);
  --card: var(--surface);            --card-foreground: var(--content);
  --popover: var(--sheet);           --popover-foreground: var(--content);
  --primary: var(--inverse-surface); --primary-foreground: var(--inverse);
  --secondary: var(--fill);          --secondary-foreground: var(--content);
  --muted-foreground: var(--muted);
  --accent: var(--selection);        --accent-foreground: var(--content);
  --destructive: var(--deletions);
  --border: var(--stroke);           --input: var(--stroke);
  --ring: var(--accent-color);
  --radius: 12px;
}
```

Exact `--muted` for shadcn = `var(--surface)` (shadcn uses `muted` as a background); the text token is `--muted-foreground`. The raw palette `--muted` above is therefore exposed to Tailwind as `text-muted`, see below, and shadcn's `bg-muted` is mapped to `surface` in the theme block.

### 5.3 Tailwind v4 `@theme`

```css
@import "tailwindcss";
@custom-variant dark (&:where([data-scheme="dark"], [data-scheme="dark"] *));

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--content);
  --color-drawer: var(--drawer);
  --color-sheet: var(--sheet);
  --color-surface: var(--surface);
  --color-surface-raised: var(--surface-raised);
  --color-stroke: var(--stroke);
  --color-selection: var(--selection);
  --color-content: var(--content);
  --color-muted-text: var(--muted);          /* text-muted-text */
  --color-faint: var(--faint);
  --color-brand: var(--accent-color);        /* bg-brand, text-brand */
  --color-brand-text: var(--accent-text);
  --color-additions: var(--additions);
  --color-deletions: var(--deletions);
  --color-caution: var(--caution);
  --color-file-ref: var(--file-ref);
  --color-inverse: var(--inverse);
  --color-inverse-surface: var(--inverse-surface);
  --color-fill: var(--fill);
  --color-thumb: var(--thumb);
  --color-rail: var(--rail);
  --color-bubble: var(--bubble);
  --color-list-card: var(--list-card);
  --color-list-divider: var(--list-divider);

  /* shadcn */
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--surface);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);

  --font-sans: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Roboto, system-ui, sans-serif;
  --font-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;

  --radius-pill: 999px;
  --radius-card: 22px;
  --radius-row: 12px;
  --radius-sheet: 32px;
  --radius-menu-card: 26px;
  --radius-dialog: 20px;
  --radius-sm: 6px;  --radius-md: 10px;  --radius-lg: 12px;  --radius-xl: 22px;

  --text-display: 2.125rem;   --text-display--line-height: 2.5rem;
  --text-headline: 1.375rem;  --text-headline--line-height: 1.75rem;
  --text-title: 1.25rem;      --text-title--line-height: 1.625rem;
  --text-h: 1.125rem;         --text-h--line-height: 1.625rem;
  --text-row: 1.0625rem;      --text-row--line-height: 1.375rem;
  --text-body: 1rem;          --text-body--line-height: 1.5rem;
  --text-label: 0.9375rem;    --text-label--line-height: 1.25rem;
  --text-small: 0.875rem;     --text-small--line-height: 1.25rem;
  --text-meta: 0.8125rem;     --text-meta--line-height: 1.125rem;
  --text-caption: 0.75rem;    --text-caption--line-height: 1rem;
  --text-micro: 0.6875rem;    --text-micro--line-height: 0.875rem;

  --shadow-*: initial;        /* no shadows: removes every shadow utility */
  --drop-shadow-*: initial;

  --ease-out-cubic: cubic-bezier(0.215, 0.61, 0.355, 1);
  --ease-standard: cubic-bezier(0.4, 0, 0.2, 1);
  --ease-word: cubic-bezier(0.33, 0.53, 0.67, 1);
  --breakpoint-sm: 40rem;     /* 640 */
  --breakpoint-lg: 64rem;     /* 1024 */
}

@layer base {
  html { font-family: var(--font-sans); color: var(--content); background: var(--background); -webkit-text-size-adjust: 100%; }
  body { margin: 0; min-height: 100dvh; overscroll-behavior: none; }
  * { border-color: var(--stroke); }
  :focus-visible { outline: 2px solid var(--accent-color); outline-offset: 2px; }
  .tabular { font-variant-numeric: tabular-nums; }
}
```

The `text-muted-text` / `bg-brand` names avoid colliding with shadcn's `muted` and `accent`. Wrap them in project aliases if preferred, but never reuse shadcn's `accent` for the brand color.

## 6. Motion

### 6.1 Springs (Reanimated -> `motion`)

`motion` `{ type: "spring", stiffness, damping, mass }` takes the same three numbers as Reanimated `withSpring`, so the values transfer directly. Validate the feel visually once (the two libraries integrate slightly differently).

| Name | stiffness / damping / mass | Use | Source |
|---|---|---|---|
| press | 100 / 15 / 0.8 | glass press scale to 1.05 and back | `LiquidView.tsx:33` |
| pop | 190 / 14 / 0.9 | glass pop-in (y 40, scale .85) | `LiquidView.tsx:60` |
| pager | 1000 / 500 / 3 (overdamped, no overshoot) | page swipe, drawer, sidebar collapse | `ScenePager.tsx:40-46` |
| mini pill | 210 / 17 / 0.8 | now-playing list items | `LiveSessions.tsx:60` |
| deck | 180 / 18 / 0.9 | finished deck spread | `FinishedSessions.tsx:44` |
| pulse pop | `animate 1.15` over 140ms, then spring to 1 | status dot | `LiveSessions.tsx:525` |

```ts
export const springs = {
  press: { type: "spring", stiffness: 100, damping: 15, mass: 0.8 },
  pop:   { type: "spring", stiffness: 190, damping: 14, mass: 0.9 },
  pager: { type: "spring", stiffness: 1000, damping: 500, mass: 3 },
  mini:  { type: "spring", stiffness: 210, damping: 17, mass: 0.8 },
  deck:  { type: "spring", stiffness: 180, damping: 18, mass: 0.9 },
} as const;
```

The pager spring reads as a ~250ms ease-out; a CSS transition `250ms var(--ease-out-cubic)` is an acceptable stand-in.

### 6.2 Durations and easings

| What | Duration / easing | Source |
|---|---|---|
| Composer pill <-> card morph | 280ms out-cubic | `Composer.tsx:65` |
| Dictation cross-fade | 180ms out-cubic | `Composer.tsx:66` |
| Collapsible height + opacity | 280ms `--ease-standard` | `Collapsible.tsx:15` |
| Segmented switch thumb | 240ms out-cubic | `SegmentedSwitch.tsx:25` |
| Transcript reveal on open | opacity 0 to 1, translateY 10 to 0, 280ms out-cubic | `Transcript.tsx:164-173` |
| Loading spinner | shows after 250ms, fade 200ms | `Transcript.tsx:57,493` |
| Jump-to-end button | opacity 160ms, 36px | `Transcript.tsx:334` |
| Header piece push | 300ms in-out cubic, 50ms stagger, 22px travel | `SceneFade.tsx:14-28` |
| Composer height growth | 180ms out-cubic | `SessionScreen.tsx:561` |
| Sheet (Android style, used on web) | backdrop 180ms fade, sheet 280ms slide-in | `ModalSheet.tsx:52,60` |
| Empty-session background | 520ms out-quad | `EmptySession.tsx:31` |
| Finished card lift | 560ms from 56px | `FinishedSessions.tsx:52` |
| Pulse burst ring | 900ms out-cubic, scale 1 to 1.7, opacity 1 to 0 | `LiveSessions.tsx:515` |
| Braille spinner | frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` every 80ms | `TerminalSpinner.tsx:7-8` |
| Copy confirmation reset | 2000ms | `TranscriptTurn.tsx:217` |
| Pressed feedback | opacity .6 (.7 settings, .4 disabled) | `SettingsUI.tsx:136` |

Streaming word reveal (`src/features/sessions/wordReveal.ts`): fade length = 3x running average gap, clamped 120..400ms, average seeded 160ms and updated `avg*0.7 + min(gap,1000)*0.3`, half-written words held up to 150ms, curve `--ease-word`. Wrap new words in spans with an opacity transition; words present at open show instantly.

Shimmer (`Shimmer.tsx`): `background-clip: text` gradient (transparent, content, transparent) animating `background-position`, 1.6s linear infinite (fold label 1s). Expose the text as the accessible name.

DitherLoading (`DitherLoading.tsx`): pre-render the 24 frames (24x24 cells of 3px, 1200ms per turn) to a sprite strip, step with `animation-timing-function: steps(24)`, `image-rendering: pixelated`, label "Loading". A plain accent spinner is an acceptable MVP.

### 6.3 Reduce motion

Mobile respects `useReducedMotion` in Ticker, DitherLoading, pulse, ASCII background (`mobile/src/ui/*`). Web:

- Wrap the app in `<MotionConfig reducedMotion="user">` (transforms and layout animations are disabled, opacity kept).
- CSS: under `@media (prefers-reduced-motion: reduce)` set transitions to `0ms`-`120ms` opacity only, stop shimmer, ticker, pulse and dither loops (static first frame or a slow opacity pulse), word reveal shows words instantly.
- Unlike mobile (pager spring ignores reduce-motion), web does not exempt the pager: under reduce it cross-fades.

## 7. Icons

Mobile uses Hugeicons core-free (`mobile/src/ui/Icon.tsx:59-112`) at size 20, strokeWidth 1.75, color `content`. Web uses `@hugeicons/react` with `@hugeicons/core-free-icons` and the same exports; wrap them in an `<Icon name size strokeWidth>` component keyed by the same logical names, so the mapping lives in one file. All export names below are copied from the mobile source and exist in the package the mobile app imports; the web wrapper API (`HugeiconsIcon icon={...}`) is "to confirm" against the installed `@hugeicons/react` version.

| Logical name | Hugeicons export | Logical name | Hugeicons export |
|---|---|---|---|
| sidebar | LayoutAlignRightIcon | copy | Copy01Icon |
| more | MoreHorizontalIcon | trash | Delete02Icon |
| search | Search01Icon | check | Tick02Icon |
| close | Cancel01Icon | refresh | Refresh01Icon |
| add | Add01Icon | metrics | ChartBreakoutSquareIcon |
| compose | PencilEdit02Icon | inbox | InboxIcon |
| chat | BubbleChatIcon | image | Image01Icon |
| settings | Settings01Icon | notifications | Notification01Icon |
| folder | Folder01Icon | note | Note01Icon |
| file | File01Icon | automation | FlashIcon |
| send | ArrowUp02Icon | bot | BotIcon |
| mic | Mic01Icon | lock | LockIcon |
| chevronDown | ArrowDown01Icon | shield | Shield01Icon |
| chevronUpDown | UnfoldMoreIcon | filter | FilterHorizontalIcon |
| chevronRight | ArrowRight01Icon | appearance | DarkModeIcon |
| branch | GitBranchIcon | sun | Sun03Icon |
| computer | ComputerIcon | moon | Moon02Icon |
| computerOff | ComputerRemoveIcon | info | InformationCircleIcon |
| sparkle | SparklesIcon | archive | Archive01Icon |
| terminal | ComputerTerminal01Icon | people | UserGroupIcon |
| list | LeftToRightListBulletIcon | layers | Layers01Icon |
| explorer | FileScriptIcon | circle | CircleIcon |
| source | SourceCodeIcon | robot | Robot01Icon |
| preview | ViewIcon | wrench | Wrench01Icon |
| wrap | TextWrapIcon | minus | MinusSignIcon |
| edit | PencilEdit01Icon | | |

New web-only icons (all "to confirm" in the installed package, pick the nearest if a name is missing):

| Need | Candidate export (to confirm) |
|---|---|
| passkey / key | Key01Icon |
| QR code | QrCodeIcon |
| invite / user add | UserAdd01Icon |
| logout | Logout01Icon |
| install / download | Download01Icon |
| link / pairing | Link01Icon |
| desktop online / offline dot | reuse `computer` / `computerOff` |
| admin | UserShield01Icon |
| external link | LinkSquare01Icon |
| cloud (Cloud page and its empty state) | CloudIcon |
| account / person (header Account entry) | UserCircleIcon (`account`) |
| split pane right / down (tab strip) | LayoutTwoColumnIcon (`splitRight`), LayoutTwoRowIcon (`splitDown`) |

Sizes in use: 22 menu leading icon, 20 default and circle buttons, 18 settings rows and chevrons, 16 fold marks, 14 inline file icon and swatch check, 11 Mono done check. Non-Hugeicons glyphs: braille spinner, provider logos as SVG (`src/features/sessions/providerSvgs.ts`), Monocode mark PNGs tinted via `mark` (use CSS `mask-image` + `background: var(--mark)`), file-type icons (`src/features/files/FileTypeIcon.tsx`).

## 8. shadcn/ui mapping

Install shadcn components with Tailwind v4, then restyle through the tokens above. Global customisations that apply to every component: radii from section 3.2, no shadows (`--shadow-*: initial` removes them; also delete any `shadow-*` class from generated code), 1px `stroke` borders, `selection` for hover and active rows, focus ring `accent` 2px offset 2px, minimum 44px hit area on touch (`@media (pointer: coarse)`), 32-36px on `pointer: fine`.

| Mobile component (source) | Web component | Required customisations |
|---|---|---|
| `LiquidView` (`src/ui/LiquidView.tsx`) | custom `<Glass>` wrapper (section 4.1) | glass CSS, press spring, hit slop, `haptic("light")` on press-in |
| `CircleButton` | shadcn `Button` variant `circle` (size 44, 36 in composer) | `rounded-pill`, bg `fill`, glass when over content, icon turns `brand` when `on`, required `aria-label` |
| Primary button (connect, send) | `Button` variant `default` | bg `primary` (= inverseSurface), text `inverse`, radius 32 for big CTA, 999 for send; NOT accent colored |
| Text/link button (usage card actions) | `Button` variant `ghost`/`link` | 14/600, text `brand-text` |
| `SegmentedSwitch` | shadcn `Tabs` (or `ToggleGroup`) restyled | glass track height 44, inset 4, thumb radius 18 color `thumb` animated with `motion` `layoutId` (240ms), unselected icon opacity .5, selected shows label, `role=tablist/tab`, arrow-key navigation, `selection` haptic |
| `ChoiceMenu` (mobile) | vaul `Drawer` on touch / shadcn `DropdownMenu` on `pointer: fine` | sheet bg `sheet`, top radius 32, handle 40x4 r2 `faint`, header 56 title 20/600, section cards r26 bg `surface-raised`, rows min 56 pad 16/12 gap 14, label 17, icon 22, check 20 accent, destructive `deletions`, divider 0.5px inset 52/16, disabled .4 |
| `ModalSheet` / bottom sheets | vaul `Drawer` | top radius 32, scrim `rgba(0,0,0,.4)`, grabber 36x5 r2.5 `faint`, drag-to-dismiss, max height 80-92dvh, bottom padding `env(safe-area-inset-bottom)+16`, `aria-modal`, focus trap (vaul uses Radix Dialog) |
| Rename / confirm dialogs | shadcn `Dialog`, `AlertDialog` | bg `sheet`, radius 20, padding 20, gap 16, scrim .4, input bg `fill` r8 pad 12 16px, actions right-aligned gap 8; destructive confirm uses `deletions`. Replaces `Alert.alert` and `ActionSheetIOS` |
| Settings card + row (`SettingsUI.tsx`) | custom `<SettingsCard>`, `<SettingsRow>` on shadcn `Card` primitives | card bg `surface`, r22, pad 10/6, rows min 52 r12, 36px icon slot, chevron 18 `faint`, dividers 1px inset 40, group heading muted 15, footnote muted 13/18 |
| Session row (drawer, `SessionCard.tsx`) | custom `<SessionRow>` | min 44, gap 12, pad 14h, r12, active bg `selection`, hover `content-a05`, title 17, working spinner in accent |
| Detailed session card | shadcn `Card` | pad 14, gap 6, title 16/600, time 13 `faint` tabular, status 13 accent |
| Drawer / sidebar (`WorkspaceDrawer.tsx`, `ScenePager.tsx`) | shadcn `Sidebar` (restyled) or custom | bg `drawer`, no border shadow, desktop persistent, mobile off-canvas via pointer-swipe; width `min(82vw, 330px)` on mobile, 320-360 on desktop |
| Swipe actions on rows | custom `motion` drag + keyboard `DropdownMenu` / context menu | 40x40 buttons gap 8 r12; add a visible "more" button for non-touch |
| `Composer` | custom (`textarea` autosize + shadcn `Button`/`Popover`) | collapsed pill height 50 r25 glass, expands to card (280ms out-cubic), input 17/22 min 64 max 160, round buttons `fill`, model pill 36 r18, send primary round, stop = 12x12 r3 square; Enter sends on desktop (Shift+Enter newline), button-only on touch |
| `QuestionCard` | custom with shadcn `RadioGroup` / `Checkbox` | options r12 1px stroke, active border `muted` + bg `selection`, radio 18 r9 / checkbox 18 r5 border 1.5 `faint`, active fill `content` |
| Switch (collapse tool calls) | shadcn `Switch` | track on = `brand` |
| Text inputs / search | shadcn `Input` | bg `fill` or `surface`, r12, 17px, 1px stroke, no ring shadow (outline only) |
| Toast / notices | shadcn `Sonner` | glass surface, r16, no shadow |
| Error notice, usage card | custom `<Notice>` | bg `content-a04` + 1px stroke r16 pad 14; usage: bg `fill` r18 pad 14 |
| Tooltip (web only) | shadcn `Tooltip` | bg `sheet`, 1px stroke, r8, 13px, only on `pointer: fine` |
| Popover (model/project/branch pickers) | shadcn `Popover` / `Command` | bg `sheet`, r22 (menu card style), list rows as menu rows; on touch becomes a vaul Drawer |
| Tabs of pages (pager) | custom swipe pager (motion drag, `touch-action: pan-y`) | pager spring; keyboard: arrow keys on tablist |
| `Collapsible` | shadcn `Collapsible` + `motion` height | 280ms `--ease-standard`, `inert` + `aria-hidden` when closed |
| Transcript, markdown | custom AST renderer ported from `markdownParser.ts` (React elements, no HTML sink) + shiki | tokens in section 2 and 1.4 |
| `Shimmer`, `Ticker`, `DitherLoading`, `GradualBlur` | custom | section 4 and 6.2 |
| Badge / unread | shadcn `Badge` | bg `#ff3b30`, text `#fff` 12/600, min 20x20 r10 |
| Table (markdown) | shadcn `Table` | 1px stroke r8, cell 10/6, 14/20, header 600, min column 140 |
| Skeleton / spinner | custom | muted spinner after 250ms |

## 9. Accessibility

Contrast measured with WCAG relative luminance on the real tokens (background `#131313` dark, `#f7f7f7` light):

| Token pair | Ratio | Verdict | Web fix |
|---|---|---|---|
| muted dark `#8c8c8c` on `#131313` | 5.5 | pass | none |
| muted light `#737373` on `#f7f7f7` / `#ededed` | 4.4 / 4.1 | fail AA | web light `--muted: #666666` |
| faint dark / light | 2.8 / 2.3 | fail | decorative only (chevrons, grabber, disabled hints); never for essential text |
| accent light `#1f7ae0` on `#f7f7f7` | 4.0 | fails for small text | `--accent-text` in light = `color-mix(in srgb, var(--accent-color) 85%, black)` (blue becomes about `#1b68c0`, 5.0); verify each accent/theme in the visual check; use plain `--accent-color` for fills, rings and large text |
| light accents teal, green, orange on `#f7f7f7` | 4.5, 4.6, 4.3 | borderline | same `--accent-text` rule |
| content alpha on dark background | .45 = 4.0, .5 = 4.6, .55 = 5.4 | .45 fails | text uses >= .5 |
| content alpha on light background | .5 = 2.9, .65 = 4.4, .72 = 5.4 | fails below ~.7 | light mode text uses >= .72 |
| user time (.4) and footer time (.35) | dark 3.4 / 2.9, light 2.3 / 2.0 | fail | raise to the same minimums above; these are the only essential-text uses that must change |

Implement the override as `:root[data-scheme="light"] { --muted: #666666; --content-a35: var(--content-a72); --content-a40: var(--content-a72); --content-a45: var(--content-a72); --content-a50: var(--content-a72); ... }` with an added `--content-a72` step, and `:root[data-scheme="dark"] { --content-a35: var(--content-a50); --content-a40: var(--content-a50); --content-a45: var(--content-a50); }`. Non-text strokes (`stroke`, `rail`) stay as is (decorative); interactive component boundaries rely on `stroke` plus a state change, and selection is never color-only (check marks, filled radio, active background).

As built, the override is three layers, all in CSS variables so components never branch on the scheme:

1. `src/styles/tokens.css`, `:root[data-scheme="light"]`: `--muted: #666666` and every text alpha step from `--content-a35` to `--content-a65` points at `--content-a72`. `:root[data-scheme="dark"]`: `--content-a35`, `--content-a40` and `--content-a45` point at `--content-a50`.
2. `src/features/themes/contrast-overrides.css` (imported after the tokens by `features/themes/register.ts`): dark raises the `--content-a50` floor to `--content-a55`, because `.50` measures 4.4 on `surface` in 5 of 6 themes. No theme or accent needs an override of its own. `contrast.test.ts` measures `tokens.css` plus this file for every theme, scheme and accent with the helpers of `contrast.ts`, so a new theme or token that falls under the minimum fails the test.
3. `@media (prefers-contrast: more)` in `tokens.css`: `--stroke` becomes `content` at 30% and every content alpha from `--content-a35` to `--content-a85` becomes the full `--content`.

To change a text colour, change the token or the override, never a component. Add a new theme only together with its row in `contrast.test.ts`.

Rules:

- **Focus**: `:focus-visible` outline 2px `accent-color`, offset 2px, on every interactive element; never remove outline without replacement. Inputs show the same ring (outline, not shadow). Mobile has no focus style, web must.
- **Keyboard**: full operation without pointer. Tab order follows visual order. Menus, popovers and listboxes: arrow keys, Home/End, Enter/Space, Esc closes and returns focus to the trigger (Radix primitives provide this; custom components must match). Segmented switch: roving tabindex, Left/Right. Pager: arrow keys on the tablist. Skip link to the transcript and composer. Shortcuts (desktop): `Cmd/Ctrl+K` search, `Esc` close, `Cmd/Ctrl+Enter` send; all discoverable and documented in settings.
- **ARIA**: icon-only buttons always have `aria-label` (mobile requires it on `CircleButton`). Sheets and dialogs: `role="dialog" aria-modal="true"` with a title (`aria-labelledby`), focus trap, return focus. Tabs: `role=tablist/tab/tabpanel`, `aria-selected`. Radio and checkbox options: native or `role=radio/checkbox` with `aria-checked`. Rows with state: `aria-selected` / `aria-current="page"`. Collapsed content: `aria-hidden` + `inert`. Streaming status and "needs you" changes: one polite `aria-live="polite"` region (not the whole transcript). Errors: `role="alert"`. Decorative spinners and braille frames `aria-hidden`; Shimmer exposes its text. Swipe-only actions need a keyboard / menu alternative.
- **Reduced motion**: section 6.3. Also honor `prefers-reduced-transparency` (glass becomes opaque `sheet`) and `prefers-contrast: more` (stroke at .3 alpha, content alphas raised to 1).
- **Text scaling**: sizes in rem, no `maximum-scale`/`user-scalable=no`, layouts reflow at 200% zoom with no horizontal scroll (except code and tables), line-heights are unitless or rem so they scale. Respect browser minimum font size.
- **Touch targets**: >= 44px on `pointer: coarse` (rows, buttons, accent ring 38 + 4px slop); 32-36px allowed on `pointer: fine`. Spacing between adjacent targets >= 8px.
- **Color**: status never color-only (icon or text accompanies success, error, working). Diff rows use a `+` / `-` gutter in addition to color.
- **Language and landmarks**: `<html lang>`, `main`, `nav` (sidebar and bottom tabs), `aside` for side panels, a single `h1` per route.

## 10. Haptics

The Vibration API does not exist on iOS Safari and installed iOS PWAs, so the iOS replacement for `expo-haptics` is a visual press response only (scale and opacity, section 4.1). Android Chrome supports `navigator.vibrate`.

Mobile haptic points (all must call the abstraction so behaviour is centralised): light on glass press-in, drawer open/close, copy, dictation start/stop; medium on session long-press and live pill; selection on segmented switch, pager change, pickers, fold toggles, theme/accent choice; success notification on session finished and saved settings (see `mobile` grep for `Haptics.` call sites).

```ts
// web/app/src/lib/haptics.ts
type Haptic = "light" | "medium" | "selection" | "success" | "warning" | "error";
const PATTERNS: Record<Haptic, number | number[]> = {
  light: 8, medium: 16, selection: 4, success: [10, 40, 10], warning: [20, 60, 20], error: [30, 50, 30, 50, 30],
};
export function haptic(kind: Haptic): void {
  if (typeof navigator === "undefined" || typeof navigator.vibrate !== "function") return; // iOS, desktop: no-op
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (!hapticsEnabled()) return;   // user setting, default on, persisted as monoptah.haptics
  navigator.vibrate(PATTERNS[kind]);
}
```

Vibration is allowed only after a user gesture, so call it from event handlers. Pattern durations are initial guesses (to tune on a device). Never rely on haptics for feedback: every haptic point has a visual change too.

## 11. Responsive layouts

Breakpoints: mobile `< 640px`, tablet `640-1024px`, desktop `>= 1024px` (`--breakpoint-sm: 40rem`, `--breakpoint-lg: 64rem`). Use container queries inside panes (mobile's `ContentPane` / `useContentWidth` measures pane width for the same reason). Routes live under `/d/:desktopId/...`; the layout adapts, the URL does not.

Information architecture, as in mobile (`ScenePager.tsx`, `WorkspaceDrawer.tsx`): Desktops (machines) > projects / sessions > transcript with composer; secondary screens: files, changes (diff), git, usage, settings.

| | Mobile (<640) | Tablet (640-1024) | Desktop (>=1024) |
|---|---|---|---|
| Desktop list / switcher | one slim row at the top of the `/d` frame: the desktop switcher (sheet) on the left, the app nav icons (Search on a phone, Desktops, Account, Install) right aligned; there is no separate header row on `/d` routes | same row | same row |
| Workspace list (Monos, projects, Cloud, settings) | off-canvas drawer (swipe from edge, width `min(82vw, 330px)`), also home when no session is open | persistent sidebar 300px (collapsible with button) | persistent sidebar 320-360px (resizable, remembered) |
| Session list of a project | the project page (its own screen) | the project page (its own screen) | a second sidebar beside the workspace sidebar, 240-560px, default 320 (resizable, `monoptah.layout.sessionListWidth`), shown on every screen of a project (project routes, a session, Files, Changes, Git); the native classic layout has the same column (260 default, 200-560) |
| Transcript | full screen, one pane at a time, no tabs | main pane, flex, with a tab strip (section 11.2) | chat pane with tabs and split panes (section 11.2): fills the rest of the width edge to edge (no card, no border, no centering of the pane); the transcript text column inside keeps max-width 768px centered |
| Composer | bottom, floating glass pill | same | same, max-width aligned with the transcript column |
| Side panels (files, changes, git, usage) | full-screen sheet (vaul, 92dvh) pushed over the transcript | right panel 360px overlay (slides over, dismissable) | right panel 380-480px docked, resizable, can stay open beside the transcript |
| Navigation | drawer + horizontal page pager between scenes, header glass buttons | sidebar + header | sidebar + header, keyboard shortcuts, no pager |
| Pickers (model, project, branch, effort) | vaul bottom sheet | popover | popover (dropdown) |
| Confirmations / rename | `Dialog` centered (radius 20), or sheet for long forms | dialog | dialog |
| Settings | full-screen routes, drawer-level entry | two-pane (list left, detail right) in a modal dialog 720px | two-pane in a dialog 800px or a full route |
| Desktop manager, admin | full-screen routes | two-pane | table-like list with detail panel |
| Pointer model | touch, swipe gestures | touch + pointer | pointer first, hover states, context menus, tooltips |

The desktop layout mirrors the native classic layout: workspace sidebar | session list sidebar | chat pane (`ChatPane`, `features/layout/ChatPane.tsx`; from 640px it holds the tabs and split panes of section 11.2) | optional docked right panel. A docked panel is sized as if the session list were at its 240px minimum and the list shrinks to make room (the stored width is untouched), so the chat pane never drops below 320px; where 380px does not fit, the panel is the overlay. The project routes (`/p/:projectId`, `/p/:projectId/new`) show the New session page in the chat pane, and the session list names the project (it holds the page's only h1; on a session or tool screen the chat holds the h1 and the list heading is an h2). Below 1024px there is no session list column: the project page and `/new` are separate pages as before.

The pages outside a desktop (`/desktops`, `/settings`, `/admin`, `/install`) keep the 44px header (a glass pill with the brand link and the same nav icons); the sign-in pages and the public `/install` show the brand only. A chat view (New session, a session, a Mono chat) draws the project's background behind everything, under a scrim of the page background colour at 50% so text keeps its contrast in light and dark; a Mono chat uses the background of the first of its projects that has one.

Rules:

- Sheets (vaul) below 640px, anchored popovers or dialogs from 640px up (switch by `useMediaQuery("(min-width: 40rem)")` plus `pointer: fine`).
- The drawer scrim is 0.5 black in dark only, 0 in light; persistent sidebars have no scrim, only a 1px `stroke` edge (no shadow).
- Transcript text column stays at the mobile type sizes on all breakpoints (do not scale type up on desktop); only the max-width changes.
- Headers 44px high with glass circle buttons 44px (36 on desktop with `pointer: fine` allowed).

### 11.1 Safe area and viewport

```html
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
<meta name="apple-mobile-web-app-capable" content="yes" />
<meta name="mobile-web-app-capable" content="yes" />
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent" />
<meta name="theme-color" content="#131313" />
```

- `viewport-fit=cover` extends the app under the notch and home indicator; pad with `env(safe-area-inset-top|right|bottom|left)`: header top, composer and sheets bottom (`max(env(safe-area-inset-bottom), 0px) + 16px`), page bottom `inset + 40px`, landscape left/right on the sidebar and transcript gutters.
- Use `100dvh` (not `100vh`) for the app shell; the keyboard on iOS shrinks the visual viewport, so size the composer from `window.visualViewport` (resize and scroll events) and keep it above the keyboard; scroll the transcript with an internal scroller, `overscroll-behavior: none` on `body`, and never let the body scroll (avoids rubber-band exposing the background).
- `black-translucent` status bar lets the page background show behind the status bar in standalone mode; set `background` on `html` to the active scheme so no white strip shows.
- Standalone iOS PWA: no browser back button, so every screen needs an in-app back control; keep edge-swipe-back off native (the pager uses pointer events); no reload gesture, so provide a pull-free "Reconnect" action; opening external links leaves the app, so mark them clearly. The WebSocket drops about 10s in background; resync on `visibilitychange` (see architecture doc).
- Inputs at 16px or larger (the composer is 17px) so iOS does not zoom on focus.
- `touch-action: pan-y` on pager scenes, `manipulation` on buttons, `-webkit-tap-highlight-color: transparent`, `user-select: none` on chrome, `text` on transcript content.
- Manifest: `display: "standalone"`, `orientation: "any"`, maskable icons derived from `mobile/assets/images/icon.png`, `theme_color` and `background_color` `#131313`.

### 11.2 Session tabs and split panes

Code: `features/layout/tabs/` (`paneTree.ts` the pure model, `tabsStore.ts` persistence, `TabbedChat.tsx` the panes, `TabStrip.tsx`, `PaneDivider.tsx`, `PaneDropZones.tsx`, `useTabActions.ts`, `tabShortcuts.ts`). `ChatPane` renders it from 640px; below that it is the plain route screen (one session at a time, no tabs, no split, nothing stored).

- **Tabs.** A tab is a session (`/d/:desktopId/s/:id`) or a Mono chat (`/d/:desktopId/monos/:id`). The New session page, Files, Changes, Git and the Mono profile are not tabs: they show in the focused pane in place of its active tab (no tab is selected), so New session behaves as a page you start from, like the native New session button. A pane with no tab and no split shows no strip, so project pages look as before. A tab whose session the catalog does not list keeps a plain name ("Session", "Mono", italic) and can be closed; the catalog may carry only recent sessions, so tabs are never dropped for being absent.
- **URL is the source of truth.** Navigating to a session or Mono (list, link, palette) opens its tab at the end of the focused pane, or focuses the pane that already has it (one tab per session in the workspace). Choosing a tab, focusing a pane or closing a tab navigates to the focused pane's active tab; closing the last tab goes to the project (or the workspace home). Only the active tab of each pane is mounted.
- **Strip.** `role="tablist"` of `role="tab"` buttons (`aria-selected`, roving tabindex, Left/Right/Home/End move focus, Enter or Space opens, Delete closes, Alt+Left/Right reorders), each with a labelled close button, then the pane's Split right, Split down and Close pane buttons. Targets are 44px on coarse pointers (36px fine). Tabs are 120-224px wide and scroll sideways (mouse wheel too); the selected tab scrolls into view. Middle click closes a tab. Drag a tab to reorder or onto another strip. Tabs and panes persist per desktop in `monoptah.layout.tabs.<desktopId>`.
- **Panes.** A tree of splits (row or column, fractional sizes, up to 6 panes). Each pane has its own tab list and active tab; the focused pane (pointer or keyboard focus inside it) drives the URL, and its strip marks the active tab with the brand colour. Dividers resize with the pointer or arrow keys (min 320px wide, 280px high per pane). While a tab is dragged, each pane shows drop zones: the middle moves the tab into the pane, an edge splits the pane and puts the tab in the new half. Split right/down from the buttons or the shortcuts opens a new empty pane and the New session page of the active session's project in it (the native split does the same: a new session beside the focused one). The last tab of a pane closes the pane. Only the focused pane's title is an h1, the others are h2.
- **Room.** A split is offered only while the result fits the chat area (two 320px panes side by side, two 280px panes stacked), measured on the chat area, so a tablet at 1024px (chat area about 700px) can split in two and one at 768px cannot. When the area becomes too narrow for the existing panes (window resize, a docked panel opening), only the focused pane shows and its strip says "+N hidden"; the others keep their tabs and come back when there is room.
- **Shortcuts** (Mod is Cmd on macOS, Ctrl elsewhere; keys match `event.code`; they do nothing under a dialog or menu):

| Action | Web | Native (`shell.rs:262-325`) |
|---|---|---|
| Split right | Mod+D | Mod+D |
| Split down | Mod+Shift+D | Mod+Shift+D |
| Close tab (the last one closes the pane) | Mod+Alt+W | Mod+W |
| Next tab / previous tab | Mod+Alt+] / Mod+Alt+[ | Mod+Shift+] / Mod+Shift+[, Ctrl+Tab |
| Tab 1 to 8, last tab | Mod+Alt+1..8, Mod+Alt+9 | Mod+1..8, Mod+9 |
| Focus next / previous pane | Mod+Alt+Shift+Right or Down / Left or Up | Mod+Alt+arrows (by direction) |

  The browser keeps Mod+W, Mod+T, Mod+N, Ctrl+Tab and Mod+1..9 and a page cannot take them, so the web moves close, next/previous and jump to Mod+Alt, and focus-pane to Mod+Alt+Shift (Mod+Alt+arrows switch browser tabs in Chrome). Mod+Alt+letters can collide with AltGr on some Windows layouts.
- **Native parity.** The native workspace keeps one pane tree per tab (`pane_layout.rs`, `split_session`, `close_pane`, drops on a pane edge); the web uses the editor-group model (each pane has its own tab list), so a drag to a pane replaces the native "tab holds a split" and there are no per-tab split trees or pinned tabs.

## 12. Checklist for new features

Before merging any UI, confirm every line:

1. Colors come only from tokens (`bg-surface`, `text-content`, `border-stroke`, ...). No hex in components, no new color without adding it to section 1 and both schemes (all 6 themes if it is a surface).
2. Works in light and dark and under every theme and accent; checked visually in at least Classic dark, Classic light and one tinted theme.
3. No shadows. Depth = surface step, 1px stroke, glass, scrim.
4. Radii from the scale (12 rows and inputs, 22 cards, 32 sheets, 999 pills); no ad-hoc values.
5. Text uses the type scale tokens in rem, system fonts, weights 400/500/600 (700 only in the session header); times and counts are `tabular-nums`.
6. Primary action is the inverse button, not accent-colored. Accent marks links, toggles, selected state and progress only.
7. Floating controls, composer and pickers use the shared `<Glass>`; edges that content scrolls under use `GradualBlur`.
8. Reuse before building: shadcn component from section 8, shared sheet/dialog/menu/choice components. Fix issues in the shared component, never per screen.
9. Overlays follow breakpoint rules: vaul sheet on mobile, popover or dialog from 640px.
10. Motion uses the shared springs and durations (section 6); respects `prefers-reduced-motion`; nothing essential depends on animation.
11. Calls `haptic()` at the same kinds of points as mobile; has a visual press state (opacity .6 or scale) as well.
12. Hit targets >= 44px on touch, visible `:focus-visible` ring, full keyboard path, `aria-label` on icon-only buttons, ARIA roles for custom widgets, contrast >= 4.5:1 for text (use the fixed tokens from section 9).
13. Safe-area insets applied to anything touching a screen edge; verified in standalone iOS PWA with the keyboard open.
14. Layout verified at 390, 768 and 1280 px widths and at 200% zoom.
15. Icons from Hugeicons through the `<Icon>` wrapper (size 20 default, stroke 1.75); add new names to the mapping table.
16. No raw HTML from agent content; markdown renders through the sanitizer-free AST renderer (port of `markdownParser.ts`, React elements only, no HTML sink); no `dangerouslySetInnerHTML` and no inline styles that violate CSP.
17. Persisted UI preferences use the `monoptah.*` localStorage prefix; nothing sensitive in localStorage.
18. Loading shows after 250ms (no flash), empty states follow the empty-session pattern (mark at .3 opacity, 22/28 headline, muted caption).
