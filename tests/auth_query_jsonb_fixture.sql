-- Fixture for the codex MED #3 BDD scenario that exercises
-- `auth_query` returning the per-user startup_parameters column as
-- native `jsonb` instead of `text`. Before MED #3, the only supported
-- shape was `::text` cast — a `jsonb` column tripped the type-mismatch
-- decoder path and was silently dropped.

DROP TABLE IF EXISTS auth_users_jsonb;
CREATE TABLE auth_users_jsonb (
    username TEXT NOT NULL,
    password TEXT,
    startup_parameters JSONB
);

SET password_encryption = 'md5';

DROP USER IF EXISTS sp_jsonb_user;
CREATE USER sp_jsonb_user WITH PASSWORD 'jsonb_pass';
INSERT INTO auth_users_jsonb
    SELECT rolname, rolpassword, '{"plan_cache_mode":"force_custom_plan"}'::jsonb
    FROM pg_authid WHERE rolname = 'sp_jsonb_user';

GRANT ALL ON DATABASE postgres TO sp_jsonb_user;
