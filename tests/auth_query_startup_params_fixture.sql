-- Fixtures for auth_query scenarios that exercise the optional
-- startup_parameters JSON column.
--
-- Layout matches auth_query_passthrough_fixture.sql with one extra
-- column. Existing scenarios that select only (username, password) keep
-- working: the new column is opt-in at the auth_query SQL level, not
-- at the table schema level.

CREATE TABLE IF NOT EXISTS auth_users (
    username TEXT NOT NULL,
    password TEXT,
    startup_parameters TEXT
);

SET password_encryption = 'md5';

-- User whose per-user startup_parameters override pool defaults.
-- Used by the passthrough-cascade scenario: pool sets plan_cache_mode
-- to 'auto', auth_query column overrides it to 'force_custom_plan'
-- for this user only.
CREATE USER sp_tuned_user WITH PASSWORD 'tuned_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword, '{"plan_cache_mode":"force_custom_plan"}'
    FROM pg_authid WHERE rolname = 'sp_tuned_user';

-- User with NULL startup_parameters: same fixture covers the
-- "column present but no per-user override" baseline.
CREATE USER sp_plain_user WITH PASSWORD 'plain_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword, NULL
    FROM pg_authid WHERE rolname = 'sp_plain_user';

GRANT ALL ON DATABASE postgres TO sp_tuned_user;
GRANT ALL ON DATABASE postgres TO sp_plain_user;
