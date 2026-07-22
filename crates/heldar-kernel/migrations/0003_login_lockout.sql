-- Per-account brute-force lockout state (complements the Worker's per-IP login rate limit).
-- failed_login_count: consecutive failed logins since the last success/unlock.
-- locked_until: RFC3339 instant before which login is refused; NULL = not locked. Auto-unlocks when
-- the window passes; cleared on a successful login, an admin edit, or POST /api/v1/users/{id}/unlock.
ALTER TABLE users ADD COLUMN failed_login_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN locked_until TEXT;
