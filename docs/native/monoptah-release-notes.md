# Monoptah Native 0.46.0

## Changes since 0.45.1

- Coding sessions linked to an internal task can work with that task's notes when you ask them to. In the session, ask for example "attach the report to the task": the agent can read the task, add a note with files (a Markdown report, a self-contained HTML result, screenshots), and edit or delete the notes it wrote. It can only reach the task the session is linked to, never your notes or a Mono's, and files must come from the session's project or worktree.
- Monos can attach files to task notes too (`today.note` takes `files`) and remove attachments when editing their own notes (`today.editNote` takes `removeAttachments`).
- Notes written by a session show the session's name instead of "You".
- Markdown and HTML note attachments open inside the app in a document viewer, with Copy and Open with default app. "Open in Monoptah" is the first item of their menu. HTML is shown in the same sandbox as Mono HTML documents, so it should be self-contained.
- Settings > Chat has "Auto resume at usage limit reset" (off by default). When it is on, a local session that hits a usage limit resumes when the limit resets, and the sidebar shows "Auto resume at HH:MM" on sessions waiting for it.
- Search in files in the Explorer shows its results again; before, the list stayed empty under the result count.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing. The in-app HTML viewer is macOS only; on Linux and Windows the viewer offers to open HTML with the default app.
