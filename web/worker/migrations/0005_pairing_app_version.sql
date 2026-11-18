-- 0005_pairing_app_version: `pair/start` receives the desktop's `appVersion` but the request row had no column for
-- it, so the new desktop row could never carry it (`desktops.app_version` stayed null). Forward-only; existing
-- rows keep null.

ALTER TABLE `pairing_requests` ADD `app_version` text;
