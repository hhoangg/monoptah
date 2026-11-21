-- 0008_desktop_avatars: the avatar of a desktop in the web activity rail and switcher (per account, so every web device
-- of the owner sees the same one). `avatar_kind` is `emoji` or `image` (null: initials). An emoji is stored as text; an
-- image is the browser's re-encoded PNG, JPEG or WebP (at most 20 KiB, magic bytes checked by the Worker) as base64
-- text in `avatar_image`, with `avatar_hash` (first 16 hex of its SHA-256) as the cache key of the image URL. The list
-- queries select explicit columns, so `avatar_image` is only read by `GET /api/desktops/:id/avatar`. Forward-only;
-- existing rows keep null.

ALTER TABLE `desktops` ADD `avatar_kind` text;
ALTER TABLE `desktops` ADD `avatar_emoji` text;
ALTER TABLE `desktops` ADD `avatar_hash` text;
ALTER TABLE `desktops` ADD `avatar_image` text;
