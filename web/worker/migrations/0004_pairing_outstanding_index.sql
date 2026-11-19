-- 0004_pairing_outstanding_index (A-07): "3 outstanding per IP" counts pending, unexpired pairing requests of one
-- IP hash (`ip_hash = ? AND status = 'pending' AND expires_at > ?`). The (ip_hash, status, expires_at) index answers
-- it from the index alone and replaces the (ip_hash) index, which it serves as a prefix. Forward-only.

CREATE INDEX `pairing_requests_ip_status_expires_idx` ON `pairing_requests` (`ip_hash`,`status`,`expires_at`);
DROP INDEX `pairing_requests_ip_hash_idx`;
