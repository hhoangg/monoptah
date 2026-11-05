# Monoptah Native 0.45.1

## Changes since 0.45.0

- Edit and delete notes on internal tasks. Edit a note in place, change its text or remove attachments, then save or cancel; an edited note shows "edited". Deleting a note asks first and also removes its stored attachment files. Notes on past days stay read-only.
- Monos can edit and delete their own task notes with `today.editNote` and `today.deleteNote`.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing.
