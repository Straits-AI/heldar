-- A site's timezone must be able to say "nobody has chosen one" (#125).
--
-- `sites.timezone` was `TEXT NOT NULL DEFAULT 'UTC'` from migration 0001. That default defeats the
-- whole resolution design: a row inserted without naming a zone comes back as 'UTC', and the
-- resolver reports it as `TzSource::Site` — "the camera's site names it" — for a zone no operator
-- ever named. A camera would then read its schedules in UTC because a row it was attached to
-- inherited a column default, which is exactly the silent shift this feature exists to prevent.
--
-- NULL now means unconfigured, and it is distinguishable from an operator deliberately choosing UTC.
--
-- Rebuilding the table rather than adding a companion column: SQLite cannot drop NOT NULL in place,
-- and `sites` has never had an insert path (no API, no writer anywhere in the tree), so on every
-- existing box this copies zero rows. Any row that DOES exist keeps its value verbatim — a hand-
-- seeded 'UTC' stays 'UTC' and stays authoritative, because we cannot tell it apart from a chosen
-- one and guessing against the operator is worse than carrying it forward.
PRAGMA foreign_keys = OFF;

CREATE TABLE sites_new (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    timezone    TEXT,            -- IANA identifier; NULL = not configured
    created_at  TEXT NOT NULL
);
INSERT INTO sites_new (id, name, timezone, created_at)
    SELECT id, name, timezone, created_at FROM sites;
DROP TABLE sites;
ALTER TABLE sites_new RENAME TO sites;

PRAGMA foreign_keys = ON;
