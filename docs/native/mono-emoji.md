# Mono emoji (native)

A Mono can add a picture emoji from the user's packs to a reply when it fits the mood.
Native only. There is ONE emoji library for the whole app, shared by every Mono:

- The library (packs, emotions, import, sorting) lives in the app Settings, page Monos, section
  "Emoji" (`crates/monocode-gpui/src/emoji_library/`, rendered by `SettingsView::monos_page`).
- Each Mono keeps only its own choices in its Settings modal, page "Emoji" after General, Soul
  and Memory (`crates/monocode-gpui/src/monos/emoji_ui.rs`, `SettingsSection::Emoji`): the
  switch, how often, and which packs of the library it may use.

## Defaults chosen for undecided points

- One emoji belongs to exactly one emotion (or Unsorted).
- Packs and emotions are global, not per Mono. A pack has no on/off switch: each Mono chooses
  which packs it uses, so one pack can serve several Monos.
- A pack that joins the library is not turned on for any Mono automatically.
- The "How often" frequency setting is kept (Rarely, Sometimes, Often), per Mono. Limits in the
  guidance: Rarely, at most one emoji, in about one reply in five, only for a clear mood;
  Sometimes, at most two per reply (a long reply may use the second where the mood or topic
  changes); Often, about one per paragraph that carries a mood, at most four per reply. In
  every mode one extra sentence lets the Mono exceed the limit when the user explicitly asks
  for emoji (for example "send several").

## Storage

The library: `<data>/emoji/`, where `<data>` is `Store::data_dir()` (the parent of
`mono::root(store)`), see `monocode_core::mono::emoji::dir` and `dir_for_root`:

```
emoji/
  emoji.json                 versioned metadata, written atomically
  packs/<pack id>/<hash>.<ext>  copied image files, named by content hash
  .staging-<uuid>/           an import in progress; removed on cancel, and when the library is first read after the app starts
  sort-<uuid>/               thumbnails of a sorting in progress; removed the same way
  (reading the library for a turn never removes anything)
```

`emoji.json` (`version: 1`, camelCase):

- `emotions`: ordered list of `{id, name, description}` (a description has at most 200 chars and one line; a name from a hand-edited file that is not valid becomes "emotion N" on load). The id is stable, so a rename keeps
  the emoji. Names are trimmed, unique case-insensitively, at most 32 chars, and may not
  contain `[`, `]` or a newline.
- `packs`: list of `{id, name, sortedBy, emoji: [{file, name, format, emotion}]}`.
  `emotion` is an emotion id or absent (Unsorted). `sortedBy` is the model label used for
  the sorting, or absent when imported without sorting.

The file has no switch, frequency or pack on/off: those are per Mono.

Per Mono, in the roster (`monocode_core::mono::Mono::emoji`, a `mono::emoji::MonoEmoji`, skipped
in the roster JSON while it is the default, so `configure_chat` and `Monos::save` carry it):
`enabled` (bool, default false), `frequency` (`rarely` | `sometimes` | `often`, default
`sometimes`) and `packs` (ids of the selected packs). An id that is not in the library is
ignored at use time. Removing a pack from the library also removes its id from every Mono and
saves the roster.

There is no migration: the earlier per-Mono folders `<data>/monos/<mono id>/emoji/` are ignored.

A file with a newer `version` is not overwritten: the page shows it read-only with a notice.
Removing a pack deletes its folder. The original zips are not needed after import.

In the app, `emoji_library::EmojiStore` (an entity installed as a GPUI global by the shell,
`emoji_library::get(cx)`) holds the `Arc<Library>`, reads it in the background, and owns the
serialized debounced writer and the edit generation. It emits `EmojiEvent::Changed` and
`EmojiEvent::PackRemoved(id)`; `Monos` listens to push the library to its chats and to drop a
removed pack from every Mono's selection.

Default emotions (name, description):

| Name | Description |
| --- | --- |
| happy | Glad, pleased, things went well |
| celebrate | A task finished, something shipped, a win |
| laugh | Something funny, a light joke |
| love | Thanks, warmth, appreciation |
| sad | Bad news, a failure, something lost |
| angry | Frustration with a bug or a broken tool |
| thinking | Investigating, weighing options |
| surprised | Unexpected result, wow |
| approve | Agree, ok, looks good |
| cool | Confident, smooth, under control |
| clever | A neat trick or a smart fix |
| nervous | Risky change, worry, deadline |
| chill | Relaxed, nothing urgent |

## Settings > Monos > Emoji (the library)

1. **Packs** card: header "N packs · M emoji · formats". One row per pack: up to 5
   thumbnails, name, counts per format ("32 emoji (29 PNG, 3 GIF)"), a pill
   "sorted by <model>" (or "not sorted"), Re-sort and Remove (confirm through the shared
   prompt; the pack goes from every Mono's selection too). "Import zip..." opens the native
   picker with `multiple: true` (GPUI has no extension filter, so non-zip picks are reported
   and skipped). Dropping one or more `.zip` files anywhere on the section starts the same
   import (shared `attachment_tray::bind_drop`).
2. **Emotions** board: title "Emotions" with "Drag emoji to move them. The description tells a
   Mono when to use the emotion." under it; on the right a pack filter (All packs, or one
   pack) and "Reset to defaults" (confirm). One card per emotion: editable name, editable
   description (placeholder "When should Mono use it?"), count, delete (its emoji move to
   Unsorted; confirm when it is not empty). Chips show the image (a GIF plays only while its chip is hovered; pack thumbnails and the import review show the first frame), with a
   tooltip "<name> · <FORMAT>". Dragging a chip onto another card moves it. "+ Add emotion"
   adds a card named "new emotion" and focuses its name. The dashed "Unsorted" card holds
   emoji no Mono uses. Reset to defaults restores the 13 default emotions; emoji whose
   emotion name matches a default keep it, the rest go to Unsorted.

The section is its own entity (`EmojiLibraryView`), so a GIF that plays on hover redraws only
it. "Manage emoji library..." in a Mono's settings closes that modal and opens this section
(`ShowEmojiLibrary` from `Monos`, handled by the shell with `open_settings(Section::Monos)`).

## A Mono's Settings > Emoji page

1. **Use emoji in replies** switch (off by default), helper "Off by default. Never in code,
   commit messages, PR text or reports." **How often** select (shared `popover::Choices`):
   Rarely, Sometimes, Often.
2. **Packs** card: "N of M packs used by this Mono" and a button "Manage emoji library...". One
   row per pack of the library: up to 5 thumbnails, name, counts, and a switch that selects
   the pack for this Mono. Under the list: "New packs are not turned on for this Mono
   automatically." An empty library shows an empty state with the same button.

Every change is saved on the Mono in the roster and handed to its chat at once.

## Import dialog

Built from the shared modal pieces in `crates/monocode-gpui/src/modal.rs` (barrier, card,
title, body, actions, buttons) above Settings.

- Title "Import emoji packs", subtitle "N files · into your emoji library" (a re-sort says
  "<pack> · your emoji library"). No Mono is named: sorting is not tied to one.
- A "Sort with" row shows the model. Its button opens the shared model picker (`ModelList` +
  `ModelPicked` in a `Menu` with `MenuItem::Custom`, as the Mono's own model button does).
  Picking a model while a zip is sorting or waits for review sorts that zip again with it.
  The dialog starts on the provider, model and account of the Mono whose settings were opened
  last (`Monos::note_settings_model`), else the app's default provider and model
  (`shell::new_chat_choice::resolve`).
- Queue: one row per selected zip (name, size, state: waiting, unpacking, sorting, review,
  saved, skipped, failed). Zips are processed one by one.
- Steps for the current zip: "Unpacked N files", "Kept N images: 29 PNG, 3 GIF (skipped:
  ...)", "Sorting into emotions with <model>... X of N" with a progress bar.
- Review grid: one box per emotion plus Unsorted with the suggested emoji, and a line like
  "1 emoji went to Unsorted." When sorting fell back to names, a notice says so.
- Buttons: Cancel (stops the whole queue, drops the staging folder, nothing more is saved;
  zips already saved stay), Import without sorting (all to Unsorted), Use this sorting. Nothing is saved until one of the last two is pressed. Then
  the next zip in the queue starts. Footer note: "Files are copied into Monoptah's data
  folder; the zip can be deleted."
- Re-sort reuses the same dialog for an existing pack (no unpack step; Use this sorting
  replaces that pack's mapping).

## Sorting with a model

- Shared one-off call: `provider::run_text` drops attachments, so a sibling in
  `crates/monocode-core/src/provider.rs` runs the same text-only, read-only, tool-less
  request with image attachments (every transport already turns `settings.attachments` into
  its own image format). No new provider client.
- Provider, model and account come from the dialog's chosen model (see Import dialog). The
  request is the same shape as before (`ProviderRunner`, text-only, read-only,
  `run_text_with_images`), with the app data folder as its working directory.
- Images are sent in batches of 16 as small PNG thumbnails (at most 96 px, first frame for
  animations), named `e01.png`, `e02.png`, ... so the reply never sees user file names.
- The prompt lists the current emotions (name and description) and asks for strict JSON
  `{"e01.png": "<emotion name>" | "unsorted", ...}`. Validation: the first JSON object is
  parsed; unknown emotions, missing files and malformed values go to Unsorted; extra keys are
  ignored.
- Runs on the background executor with an `Arc<AtomicBool>` cancel flag; Cancel and closing
  the dialog stop it. Progress is reported per batch.
- There is no per-model image capability flag in the catalog. A provider that has no
  one-off text support (`session_title::supports(kind)` false), or a call that fails or
  returns no usable JSON, falls back to a name-based suggestion (keywords from the file name
  matched against emotion names, descriptions and a small synonym table) and the dialog
  says "<model> cannot sort by image, so emoji were suggested from their file names." (no
  runner) or "<model> could not sort by image, so ..." (the first batch failed or returned no
  usable JSON). A later batch that fails leaves its emoji in Unsorted. The names fallback
  suggests nothing for a file name that matches no emotion (it stays Unsorted).

## Mono usage

- When the Mono's switch is on and at least one emotion has an emoji in one of the packs the
  Mono selected, `mono::context` gets an `<emoji>` block (inside `<monocode_context>`, after memory): the
  usable emotion names with descriptions (only emotions with emoji in the selected packs, no
  image list), the frequency guidance, and the
  syntax: write `:emo[<emotion>]` inline or alone on its line; never in code, code blocks,
  commands, commit messages, PR text, reports, files or tool input. The context fingerprint
  in `runtime.rs` resends it when it changes.
- Rendering, Mono chat transcripts only (`TranscriptView` in Mono mode with the shared library
  and the Mono's selected pack ids set, pushed by `Monos` after the library loads or changes,
  when the Mono's switch or selection changes, and after a chat is configured; the push is
  skipped when what the chat draws (switch, emotion names, the selected packs' emoji) did not
  change, so a description edit never lays the chat out again; habit and workflow run
  transcripts get it too when the library is already loaded): after markdown parsing, plain runs (not code, math, links) are scanned for
  `:emo[<name>]`. The name matches an emotion case-insensitively. The pick is a random emoji
  of that emotion across the Mono's selected packs, seeded from the session id, a hash of the message's
  raw source text before the token and the token index within the message. It does not use
  row numbers, so a re-render, a history page or an app restart shows the same picture, and
  text streaming in after the token does not change it. Two messages with the same text
  before a token get the same picture. A token inline is drawn at text size (20 px); a token
  alone on its line (only a quote or list marker before it, nothing after) is drawn larger
  (56 px). GIFs animate (in settings they play on hover only, in chat always). While a reply streams, a token that ends the text so far counts as inline until a line break follows it. Hovering an emoji (inline or large) shows a floating tooltip with a 112 px preview of the same picture (aspect kept, a GIF animates) above its "<emoji name> · <emotion>" label (`EmojiPreviewTip` in `markdown.rs`, a plain GPUI tooltip, placed just below the emoji or above it near the window bottom). Nothing is drawn, and no
  space is left, for: the switch off, an unknown or empty emotion, an emotion with no emoji
  in the selected packs, an id the library does not have, an image file that is gone, or a library that is not loaded yet.
  Tokens inside code blocks and inline code stay as text.
- The token is an empty-text run, so selection copy and search see nothing. "Copy response",
  "Save as note", "Quote response" and outward reply text (Telegram, phone) strip tokens with
  a code-aware stripper in core.
- While streaming, an unfinished trailing `:emo[...` is hidden.

## Zip import security

Untrusted input. The importer in `crates/monocode-core/src/mono/emoji/import.rs`:

- Never uses an entry path as a destination: files are written as `<sha256>.<ext>` into the
  staging folder, so zip-slip cannot escape. Entry names are still checked: absolute paths,
  `..` components, drive prefixes and backslash tricks are skipped.
- Skips symlinks (unix mode), directories, `__MACOSX/`, hidden files (any component starting
  with `.`), and nested archives (zip, rar, 7z, tar, gz).
- Limits: the zip file itself at most 200 MB, at most 500 entries, each file at most 10 MB,
  total uncompressed at most 200 MB. Sizes are enforced on the bytes actually read (a reader
  capped at the limit plus one), not on the header, which an attacker controls.
- Before the zip library parses anything, the importer reads the last end of central directory
  record itself and refuses a declared entry count (or a zip64 count) above the entry limit.
  The whole archive is then read through a wrapper that counts bytes (at most 2 x the file size
  plus 1 MiB across all reads, which also stops overlapping-entry and many-candidate tricks)
  and checks the cancel flag on every read.
- Detects the format by magic bytes, never by extension: PNG, JPEG, GIF and WebP. GPUI's
  image loader decodes more (BMP, ICO, TIFF, TGA, QOI and others, plus SVG for anything
  unrecognized) but emoji packs use these four, and SVG is excluded on purpose.
  Static images are fully decoded (a valid header over a broken body is rejected). Dimensions
  are capped at 1024 x 1024 and animations at 300 frames. GPUI decodes every GIF and animated
  WebP frame at the logical screen (canvas) size and keeps all of them, so
  `width x height x frames` (screen size, checked arithmetic) must also stay within
  `max_decoded_pixels` (16,777,216, about 64 MB of RGBA). GIFs are walked block by block
  without decompressing pixel data; a frame that does not fit inside the screen is rejected.
  Animated WebP frames are counted from the `ANMF` chunks before any frame is decoded.
  Decoding runs under `catch_unwind`, so a decoder panic only skips that entry.
- Dedupes by SHA-256 within the zip and against emoji already in the library.
- Sanitizes display names: file stem, emoji.gg numeric prefix removed, control characters
  and path separators dropped, Unicode format characters (bidi controls, zero-width, BOM, soft
  hyphen, tags) and all combining marks dropped after NFC composing (also in the skipped-entry
  display), at most 64 chars, fallback "emoji".
- Extracts to `.staging-<uuid>` inside the library folder `<data>/emoji` (same filesystem), then
  moves the files into `packs/<pack id>` when the user confirms (the target is created with
  `create_dir` as an exclusive lock, an empty import is refused, a failed move is rolled
  back), then writes `emoji.json` atomically. A failure, cancel or panic during staging removes
  the staging folder (RAII guard); `discard` only touches `.staging-*` folders.

## Tests

Tiny generated fixtures only (images built with the `image` crate, zips built with the
`zip` writer in the test). No real model call: the sorting call is faked with the existing
fake provider script pattern (`provider/text_tests.rs`) or an injectable runner factory on the library view (`EmojiLibraryView::set_sorter`).
The library page and the dialog are tested in `monos/emoji_library_tests.rs`, a Mono's own
page and how it reaches the chat in `monos/emoji_tests.rs`.
GPUI tests use `secondary-` keystrokes and no macOS-only paths.
