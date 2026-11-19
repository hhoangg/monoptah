# Monoptah web app: reuse of the Tauri React app

Status: specification, researched 2026-11-17 at commit `32738239`. Paths are relative to the repo root. LOC = `wc -l`. Ticket IDs are in [roadmap.md](roadmap.md), feature IDs in [features.md](features.md).

## 1. Rule (decided)

The old Tauri React app (`src/`, with `src-tauri/`) may be reused, but its source will be deleted later. Therefore:

1. Anything reused is **copied** into `web/app` or `web/protocol` and adapted there. It is never imported from `src/` or `src-tauri/`.
2. Nothing under `web/` (vite config, tsconfig `paths`/`references`/`include`, eslint, vitest, Tailwind `@source`/content globs, pnpm workspace, CSS `@import`, scripts) may reference `src/` or `src-tauri/`. A CI guard enforces this (roadmap.md W-20).
3. Each copied file starts with its license header (if the original has one) and a provenance comment: `// Copied from src/<path> at <commit>` (short hash of the commit the copy was taken from). The comment stays after adaptation so a later reader knows what diverged.
4. Fork branding (repo `CLAUDE.md`): user-visible text says Monoptah, never MonoCode. Do not carry `monocode.*` localStorage keys or `open-monocode` menu ids into the web app. New web keys use the `monoptah.` prefix (design-system.md sections 8 and 12).
5. Every copy removes Tauri APIs (`@tauri-apps/*`, `invoke`, `convertFileSrc`, `platform/tauri/*`), is restyled to the design-system tokens and `@hugeicons/react`, and contains no raw HTML sink (security.md section 8).

## 2. What is not taken from `src/`

- **Markdown renderer.** The web renderer is a port of `mobile/src/features/sessions/markdownParser.ts` to an AST rendered as React elements with no HTML sink (security.md section 8). The streamdown, react-markdown, rehype and `rehype-harden` stack of `src/` is NOT copied as the renderer. From it only pieces that fit are reused: the shiki highlighting engine and config, and the code block UI, rendered from `codeToTokens` tokens as React elements (never `codeToHtml`, never `dangerouslySetInnerHTML`).
- **Transcript projection.** Not reused from `src/`: the desktop already projects turns for mobile (`crates/monocode-gpui/src/mobile/transcript.rs`, 1386 LOC, a Rust port of `src/features/mobile/model/mobileTranscript.ts:137`; shape in `mobile/src/features/workspace/types.ts:184`). The web `Turn` renderer ports `mobile/src/features/sessions/TranscriptTurn.tsx`.
- **Mermaid** (`src/features/sessions/ui/AgentMarkdown.tsx:996`, `dangerouslySetInnerHTML={{ __html: svg }}`) is dropped.
- **FileTypeIcon raw SVG** (`src/features/files/ui/FileTypeIcon.tsx:78`, `dangerouslySetInnerHTML={markup}`) is replaced: hugeicons per file kind, or SVG shipped as build-time React components, never an HTML string.

## 3. Candidates by area

Columns: Source = `path:line` of the main export; Deps = what must be stripped; Shape = OK (same data as the desktop sends mobile), ADAPT (adapter needed), n/a (pure util); Val = value H/M/L. "Ticket" is the roadmap ticket that does the copy.

### 3.1 Code highlighting and markdown pieces

| Source | What | LOC | Deps to strip | Shape | Adaptation | Target in `web/` | Val | Features | Ticket |
|---|---|---|---|---|---|---|---|---|---|
| `src/features/files/editor/codeHighlightEngine.ts:56` `createBoundedCodePlugin`, `codeHighlightPlugin.ts:135` `requestCodeHighlight`, `:164` `boundedCode`, `codeHighlight.worker.ts` | Bounded Shiki highlighter (size caps, JS regex engine, Web Worker) | 215 + 176 + 28 | `@streamdown/code` plugin wrapper (call shiki `createHighlighterCore` and `codeToTokens` directly); `shiki/langs` narrowed to a curated set (PWA precache budget, capacity.md) | n/a | Output is tokens, not HTML. One dark and one light theme from design tokens (mobile uses `github-dark`, `src/features/mobile/model/mobileHighlight.ts:11`) | `app/src/lib/highlight/` | H | X-04, F-44 | W-16 |
| `src/features/sessions/ui/HighlightedCodeBlock.tsx:105` `HighlightedCodeBlock` | Code block: header, copy, numbered lines, per-line token memoisation | 243 | `streamdown` (`CodeBlockContainer`, `CodeBlockHeader`) replaced by own shadcn-style container; `./transcriptVisibility`; imports `codeHighlightPlugin` | OK | Render each token as `<span style={{color: var(--...)}}>` from CSS variables; keep copy button and line numbers | `app/src/features/transcript/CodeBlock.tsx` | H | X-04, F-44 | W-16 |
| `src/styles/index.css` (4360 LOC), only `.agent-markdown` rules (about lines 1767-2130) and keyframes `word-fade-in` (`:3666`), `shimmer-text` (`:3116`) | Markdown typography slice | about 360 | `[data-streamdown=...]` selectors (no streamdown), desktop `content/NN` colour scale | n/a | Rewrite selectors for the AST renderer classes, retarget colours to design tokens. Do not copy the file wholesale | `app/src/styles/markdown.css` | M | F-44, F-46 | W-16, U-14 |
| `src/features/sessions/ui/wordFade.tsx:131` `useStreamedText`, `:167` `usePacedText` | Paced text reveal hook | 394 (file) | `settings/model/displayPrefs` | n/a | Only if `mobile/src/features/sessions/wordReveal.ts` (181 LOC, the F-46 port target) lacks something. `rehypeWordFade` and `settleWordFades` are rehype-based and NOT copied. Respect `prefers-reduced-motion` | `app/src/features/transcript/` | M | F-46 | U-14 |
| `src/features/sessions/ui/TranscriptJumpToBottom.tsx:5,25` | Jump-to-bottom button and hook | 52 | `shared/ui/icons` | n/a | Threshold 160 px (F-33) | `app/src/features/transcript/` | L | F-33 | W-15 |
| `src/features/sessions/ui/TerminalSpinner.tsx:16`, `src/shared/ui/Shimmer.tsx:38` | Working spinner, shimmering text | 36 + 38 | none | n/a | Prefer `mobile/src/features/sessions/TerminalSpinner.tsx` for parity; copy `Shimmer` | `app/src/ui/` | L | F-24, F-36 | W-15 |

### 3.2 Question, task, plan, usage

| Source | What | LOC | Deps to strip | Shape | Adaptation | Target | Val | Features | Ticket |
|---|---|---|---|---|---|---|---|---|---|
| `src/features/sessions/ui/QuestionForm.tsx:25` `QuestionForm` + `src/features/sessions/model/userQuestion.ts:36-133` (`questionIsComplete`, `questionAnswersComplete`, `buildQuestionReply`, `selectedAnswerLabels`, `isCustomSelection`, `isOtherOption`, `CUSTOM_OPTION_ID:3`) | Multi-step question card, "Other" text, skip | 396 + 247 | `shared/ui/icons` only; drop `questionsFromUnknown` (harness parsing; the desktop sends normalised data) | ADAPT, small: `UserQuestionPrompt` (`userQuestion.ts:20`) equals mobile `QuestionPrompt` | Restyle `content/NN` classes to tokens. Add the 60 s countdown and `keepQuestionOpen` on first touch (F-58) from `mobile/.../QuestionCard.tsx`, not present here | `app/src/features/transcript/QuestionCard.tsx`, `protocol/src/question.ts` | H | F-58, F-28 | W-17 |
| `src/features/sessions/ui/TaskListPreview.tsx:10` + `model/taskList.ts:3-82` (`normalizeTaskListStatus:39`, `taskListProgressLabel:82`) | Todo checklist | 104 + 114 | `model/session` types; drop `taskListFromToolInput`, `legacyTaskListFromText` | ADAPT: desktop sends no tasks today (X-03) | Define item type in protocol | `app/src/features/transcript/TaskList.tsx` | M | X-03 | U-20 |
| `src/features/sessions/ui/PlanPreview.tsx:18` + `model/plan.ts:68-111` (`planTitle`, `planSummary`, `planMeta`) | Plan card | 127 + 127 | `SecondOpinionButton`, `skills/model/skills` (`PLAN_COMMAND`); `plan.ts:9` `source: "monocode"` | ADAPT (needs X-02 method) | Pure helpers and layout only; plan text through the shared safe renderer | `app/src/features/transcript/PlanCard.tsx` | M | X-02 | U-20 |
| `src/features/sessions/ui/UsageLimitNotice.tsx:9` + `model/usageLimit.ts:60,81-133` (`formatUsageLimitReset`, `validateMobileUsageLimit`) | Usage-limit card helpers | 157 + 152 | `providers/model/rateLimits` | OK (`MobileUsageLimit` equals `UsageLimitState`) | Copy `formatUsageLimitReset` and the validator only; base the card on mobile `UsageLimitCard.tsx` | `app/src/features/sessions/` | L | F-55 | W-18 |

### 3.3 Models, access, composer helpers

| Source | What | LOC | Deps to strip | Shape | Adaptation | Target | Val | Features | Ticket |
|---|---|---|---|---|---|---|---|---|---|
| `src/features/sessions/model/models.ts:23` `AgentModel`, `:9` `ModelSettingChoice`, `:14` `ModelSetting`, `:538-600` (`defaultModelSettings`, `mergeModelSettings`, `modelEffortSetting`, `modelEffortLabel`), `:638` `encodeModelLaunchId`, `:484` `modelContextWindow` | Model types and effort helpers | about 150 of 1050 | Static `MODELS` table (`:40-210`, stale), harness availability imports, `monocode.*` storage keys (`:225-231`: `monocode.favoriteModels`, `monocode.lastModel`, ...) | ADAPT: the live catalog comes from `models.list` | Types and helpers only. Web keys use `monoptah.` | `protocol/src/models.ts` | M | F-51, F-52 | W-18 |
| `src/features/sessions/ui/ModelPicker.tsx` | Model picker (tabs, favorites, search, settings) | 1803 + 175 css | `platform/tauri/platform`, `Popover`, `useLockOverscroll`, `HarnessIcon` wiring, `.monocode-terminal` class (`:546`) | ADAPT heavy | Do not copy. Keep structure and effort-slider semantics as reference; build on shadcn Command + vaul, porting `mobile/src/features/models/ModelPicker.tsx:229` | `app/src/features/models/` | M | F-51, F-52 | W-18 (reference only) |
| `src/features/sessions/ui/AccessPicker.tsx:40` + `RuntimeMode` (`model/session.ts:413`) | Access mode picker | 191 | `Popover` becomes shadcn DropdownMenu | OK (same four modes) | Copy labels and descriptions; remove `full-access` choice (H-03); closer source is mobile `access.ts:9` | `app/src/features/models/AccessPicker.tsx` | M | F-53 | W-18 |
| `src/features/sessions/model/session.ts:23` `HarnessId`, `:255` `Attachment`, `:413` `RuntimeMode` | Type unions only | 3 types of 891 | everything else couples to inbox, notes, orchestration | OK | Copy the unions | `protocol/src/session.ts` | M | F-23, F-53 | W-18 |
| `src/features/sessions/model/contextUsage.ts:17-64` (`formatTokens`, `contextPercent`) | Token and context formatters | 85 | none | n/a | Only if the protocol later sends context usage (P2) | `app/src/lib/format.ts` | L | F-39 | none (P2) |
| `src/features/sessions/model/attachments.ts` (`MAX_ATTACHMENTS:5`, `filesFromClipboard:199`, `isVisionImage:489`, `normalizeImageMime:493`) + `ui/AttachmentChip.tsx:15` | Clipboard to attachment, MIME helpers, chip | 547 + 92 | `platform/tauri/fs`, `invoke`; `ImageLightbox` (use shadcn Dialog) | ADAPT: web limits are 10 files and 8 MiB (F-48), the base64 shape follows `mobile/src/features/sessions/attachments.ts` | Copy the three helpers; rewrite the rest with `FileReader` and canvas resize; restyle the chip | `app/src/features/composer/attachments.ts` | M | F-48, X-08 | W-19 |

### 3.4 Shared utilities and assets

| Source | What | LOC | Deps to strip | Shape | Adaptation | Target | Val | Features | Ticket |
|---|---|---|---|---|---|---|---|---|---|
| `src/shared/ui/icons.tsx` | Hugeicons registry (about 130 icons) + `Icon` wrapper | 330 | none (`@hugeicons/react`, `core-free-icons`) | n/a | Keep only icons the web uses | `app/src/ui/icons.tsx` | H | all | W-15 |
| `src/shared/lib/paths.ts:7-80,284-331` (`slash`, `pathKey`, `parentPath`, `joinPath`, `displayPath`, `projectName`), `:195` `resolveWorkspaceFileReference` | Path helpers, file-reference resolver (`src/a.ts:12` to path and line) | 333 | `platform/tauri/platform` (`isWindows`, `setHomeDir` `:157`): make `isWindows` a parameter, drop home dir | ADAPT (Windows desktops send `\`) | Needed to open `project.file` links | `app/src/lib/paths.ts` | H | F-45, F-60, F-64 | W-15 |
| `src/features/files/model/markdownFileLinks.ts` | File-link detection | 30 | `shared/lib/paths` | OK | Works on AST text nodes, not remark nodes | `app/src/features/transcript/fileLinks.ts` | M | F-45 | W-16 |
| `src/features/sessions/ui/HarnessIcon.tsx:61` + `src/assets/providers/*.svg` (11 files) | Provider logos for 11 harnesses | 146 + svgs | Vite `?url` imports are fine | OK (same 11 ids as mobile `Harness`) | Render as `<img>` or build-time React components, never injected markup. Keep the trademark note of `NOTICE` | `app/src/assets/providers/`, `app/src/ui/HarnessIcon.tsx` | M | F-23, F-24, F-51 | W-15 |
| `src/shared/lib/fuzzy.ts:9,63`, `src/shared/ui/MatchText.tsx:10` | Fuzzy match with highlighted positions | 81 + 29 | none | n/a | Quick open over `project.search` (F-61) | `app/src/lib/fuzzy.ts`, `app/src/ui/MatchText.tsx` | M | F-61 | U-05 (copy lands first in U-15, which U-05 depends on) |
| `src/shared/lib/colorUtils.ts` | Hex, hsv, rgb conversions | 94 | none | n/a | Validate hex before any style use | `app/src/lib/color.ts` | L | F-16, F-78 | W-15 |
| `src/features/files/model/fileName.ts:60` `leafName` + `FileTypeIcon.tsx:44` | File kind and icon | 63 + 117 | raw SVG sink at `:78` (replace, section 2) | n/a | Map extension to a hugeicon or a build-time React icon | `app/src/lib/fileKind.ts` | M | F-59, F-60, F-64 | U-15 |
| `src/features/source-control/model/unifiedDiff.ts:61` `foldUnifiedLines`, `:107` `revealedFold`, `:121` `expandFold` | Diff context folding | about 150 of 236 | `@codemirror/merge`, `@codemirror/state` (`buildUnifiedFile`, `lineDiff.ts`) | ADAPT: mobile `DiffLine` equals `UnifiedLine` plus `runs` | Fold and reveal logic only | `app/src/features/changes/diffFold.ts` | M | F-68 | U-15 |

### 3.5 Stores and hooks

Verdict per item (verified in `src/`). Copy items go to ticket W-21 (roadmap.md).

| Source | What | LOC | Verdict | Reason |
|---|---|---|---|---|
| `src/features/sessions/ui/useReplyReflowAnchor.ts:12` `useReplyReflowAnchor` | Keeps the reader's character in view through reflow and reply completion | 244 | **Copy** (adapt) | Directly relevant to the W-07 iOS scroll risk. Replace the `layoutTransition` and `useLockOverscroll` imports (`:2-3`) with local no-ops or small equivalents; the caller supplies `scroller`, `following`, `remember`, `adjustments` |
| `src/features/sessions/model/sessionFilters.ts:93,102,112,129,37` (`filterSessionsByHarness`, `filterSessionsByTime`, `filterSessionsByStatus`, `timeFilterStart`, `harnessesInSessions`) | Pure sidebar filters (U-10, F-23) | 147 | **Copy** the five pure functions | Replace `SessionSummary` (`data/sessionStore`) with the protocol session card; drop `loadSessionSidebarFilters` and `saveSessionSidebarFilters` (keys `monocode.sessionSidebarFilters` `:35`, `monocode.sessionsShowArchived` `:48` are not carried; web keys use `monoptah.`) |
| `src/shared/hooks/useStableValue.ts:43` `useStableValue`, `:4` `plainEqual` | Keeps object identity when a watch frame resends equal data | 50 | **Copy** | Only React; limits re-renders of the transcript on repeated frames |
| `src/shared/lib/numbers.ts:6` `formatInteger` | Thousands separators for counts | 8 | **Copy** | Pure; used by usage meters |
| `src/features/sessions/model/sessionStateStore.ts:16` `SessionStateStore`, `:83` `useWorkspaceSessionState`, `:97` `useSessionSnapshot` | External store over the desktop `Session` array | 137 | Not reused | Bound to the desktop `Session` type and its text delivery. The web store is built on protocol types (architecture.md 3.2) |
| `src/features/sessions/data/sessionStore.ts` (1666), `sessionCache.ts`, `sessionHistory.ts` | Session persistence and history on Tauri storage | 1666 + | Not reused | Tauri persistence and `monocode.*` keys. The web keeps an encrypted persisted cache (W-12) |
| `src/features/sessions/model/transcriptFind.ts:17` `findTranscriptBlocks` + `ui/TranscriptFind.tsx:25` | Find in transcript | 32 + 250 | Not reused | Walk desktop `Block` (`block.tool.preview`); mobile turns are a different shape and find is not in the feature list |
| `src/shared/lib/remotePaths.ts:2` `REMOTE_PATH_PREFIX` | The `remote://` cwd scheme | 2 | Not reused | The web never sees `remote_cwd` (architecture.md 6.6); it gets `target.kind` |
| `src/shared/hooks/useAnimatedReorder.ts`, `useSortable.ts`, `useDragResize.ts`, `useLockOverscroll.ts`, `useColorScheme.ts` | Desktop drag, reorder and chrome hooks | n/a | Not reused | Desktop chrome (see section 4); the web uses its own theme store and shadcn or vaul behaviour |

### 3.6 Tool-call rendering

The desktop already projects tool calls into `work` turns with phases and steps (`mobile/src/features/workspace/types.ts:173-182`, `{type: "work"}` at `:208`). The web renders them by porting `mobile/src/features/sessions/TranscriptTurn.tsx` (section 2). Nothing here is copied from `src/`.

| Source | What | Verdict | Reason |
|---|---|---|---|
| `src/features/sessions/model/monocodeToolCall.ts:85` `monoCodeToolCall`, `:120` `monoCodeWorkSummary` | Labels for the Monoptah self-API tool calls | Not reused | Reads desktop `Block.tool`; the projection already names steps |
| `src/features/sessions/ui/ToolDiffPreview.tsx:19` | Inline diff of an edit tool call | Not reused | Needs data mobile turns do not carry (section 4) |
| `src/features/sessions/ui/useToolDetail.ts:30` `useToolDetail`, `:6` `ToolDetailSession` | Lazy fetch of omitted tool output through `loadToolDetail` | Not reused | Tauri session store call; mobile steps carry only `label` and `failed` |
| `useCollapsedToolCalls` (`@/features/settings/toolCalls`, used by mobile `TranscriptTurn.tsx:7`) | Collapse preference | Not reused | Already a mobile setting; the web keeps its own `monoptah.` preference |

## 4. Not reused

| Item | Reason |
|---|---|
| `src/platform/tauri/*`, `invoke`, `convertFileSrc`, `@tauri-apps/*` | Desktop IPC. The web gets data over the E2E link and uses `navigator.clipboard` |
| `AgentMarkdown.tsx` (1137), streamdown, `rehype-harden`, `hardBreaks.ts`, `streamingMarkdown.ts`, `wordFade.tsx` rehype parts | Replaced by the AST renderer (section 2). `remend`-based partial-markdown closing is not needed unless W-07 finds the mobile parser does not handle unterminated text |
| `AgentTranscript.tsx` (5438), `transcriptActivity.ts` (1229), `inFlight.ts`, `HistoryTurn`, `TranscriptPool`, `mobileTranscript.ts` | The desktop projects turns already (section 2). `INTERRUPT_MESSAGE` must never be renamed (CLAUDE.md) |
| `ToolDiffPreview.tsx`, `ApprovalToasts.tsx`, `useToolDetail.ts`, `MessageQueue.tsx` | Need data the mobile turns do not carry, or desktop queue semantics. X-01 gets its own UX (U-08) |
| `Composer.tsx` (2651), `ComposerRunner.tsx` (`innerHTML` at 147, 405), `ModelSettings.tsx`, slash commands, `btw`, `handoff`, `compact`, `orchestrator` | Desktop-only features not in `APP_METHODS` |
| `UnifiedDiffView.tsx` (1000), `FilePreview.tsx`, `BinaryFileView.tsx`, `FileTree.tsx`, `FileEditor.tsx`, `FilePicker.tsx`, `src/features/files/editor/*` | Tauri fs and CodeMirror; files and diffs arrive pre-highlighted. Port mobile screens instead (`mobile/src/features/changes/DiffScreen.tsx:25`) |
| `src/features/source-control/model/gitGraph.ts`, `GitHistoryGraph.tsx` | No history method; `gitGraph.ts:2` carries a Microsoft MIT header |
| `src/app/shell/*`, `PaneHeader`, `NativePopupHost`, `GlassBackdrop`, `Popover`, `Modal`, `layers`, `useLockOverscroll`, `nativeContextMenu.ts`, `csp.ts`, `ImageLightbox.tsx`, `useColorScheme.ts` | Desktop chrome; web uses shadcn Dialog, Popover, vaul and its own theme store |
| `src/features/terminal/*`, `mail`, `inbox`, `notes`, `contributors`, `telegram`, `automations`, `workflows`, `skills`, `orchestration`, `artifacts`, `providers`, `settings`, `connections`, `search`, `workspace`, `quick-composer`, `monos/*`, mascots | Desktop product areas outside the web feature list. `mail/ui/MailMessageBody.tsx` and `mail/model/mailQuote.ts` use `innerHTML` |
| `src/features/mobile/*` (`mobileWorkspace.ts`, `mobileWatch.ts`, `mobileHighlight.ts`, `PairPhoneDialog`) | Old TS host of the mobile protocol, now Rust. Reference only |
| `src/shared/lib/jsonText.ts`, `drag.ts`, `src/debugMonoScroll.ts`, `src/main.tsx` | Not needed, debug code, Tauri bootstrap |

## 5. License and branding notes

- Root `LICENSE` is MIT ("Copyright (c) 2026 Nick", upstream `hardbeat920/monocode`). MIT requires keeping the notice in copies: `web/` ships under the root notice (add `web/LICENSE` if the package is published on its own) and keeps a THIRD-PARTY note for copied SVG logos. `NOTICE` (provider names and logos are trademarks of their owners) applies to the provider SVGs.
- First-party files in `src/` have no per-file header. The only header is Microsoft's MIT header in `src/features/source-control/model/gitGraph.ts:2`, which is not copied. If any copied file has a header, keep it above the provenance comment.
- Provenance comment on every copied file: `// Copied from src/<path> at <commit>`.
- Branding hits found in candidates: `AgentMarkdown.tsx:191,197,199,766` ("open-monocode", `canOpenInMonoCode`; not copied), `plan.ts:9` `source: "monocode"` (dropped with `PLAN_COMMAND`), `models.ts:225-231` `monocode.*` localStorage keys (not carried), `ModelPicker.tsx:546` and `nativeContextMenu.ts:32` `.monocode-terminal` (not copied), `drag.ts:2` (not copied). No user-visible "MonoCode" copy was found in the other candidates. After copying, run `./scripts/check-fork-branding.sh` and check whether it covers `web/` (untested); extend its scan or the CI guard if it does not.
- Raw HTML sinks found in `src/` (`AgentMarkdown.tsx:996`, `FileTypeIcon.tsx:78`, `ComposerRunner.tsx:147,405`, `editorGit.ts:1152`, `mail/*`) must not reach `web/`. `web/app` ESLint bans them (W-01) and the CI guard (W-20) also greps copied files.

## 6. Copy order

1. W-15: icons, paths, provider logos, small utilities.
2. W-16: highlight engine and code block (needed by W-07).
3. W-17: question card. W-18: model types, access picker. W-19: attachment helpers.
4. W-20: CI guard (before the first copy lands, so it is in place from the start). W-21: scroll anchor hook, stable value, session filters, `formatInteger` (needed by W-07 and U-10).
5. Later: U-14 (word reveal, markdown CSS), U-15 (file kind, diff fold, fuzzy; before U-05 and U-06), U-20 (task and plan). Everything else is re-ported from `mobile/` (same data shapes, RN to DOM), which is cheaper than adapting `src/`.
