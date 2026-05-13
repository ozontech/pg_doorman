-- Fixture for auth_query rows that return per-user startup_parameters
-- as native jsonb. This covers the path where pg_doorman decodes the
-- column by PostgreSQL type instead of requiring a ::text cast.

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
