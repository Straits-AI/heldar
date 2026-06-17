-- Drop the orphaned `app_state` key/value table. It was the persistence for the legacy single-URL
-- alerting notifier, which has been superseded by webhook subscriptions (migration 0019). Nothing
-- writes it and nothing reads it after this release. The events index from migration 0002 is unaffected.
DROP TABLE IF EXISTS app_state;
