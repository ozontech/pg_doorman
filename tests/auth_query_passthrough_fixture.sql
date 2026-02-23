-- Fixtures for auth_query passthrough mode tests
-- Creates real PG users and auth_users table with their hashes

CREATE TABLE IF NOT EXISTS auth_users (
    username TEXT NOT NULL,
    password TEXT
);

-- MD5 passthrough user
SET password_encryption = 'md5';
CREATE USER pt_md5_user WITH PASSWORD 'md5_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'pt_md5_user';

-- Second MD5 user (for multi-pool test)
CREATE USER pt_md5_user2 WITH PASSWORD 'md5_pass2';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'pt_md5_user2';

-- SCRAM passthrough user
SET password_encryption = 'scram-sha-256';
CREATE USER pt_scram_user WITH PASSWORD 'scram_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'pt_scram_user';

-- Grant all users access to postgres database
GRANT ALL ON DATABASE postgres TO pt_md5_user;
GRANT ALL ON DATABASE postgres TO pt_md5_user2;
GRANT ALL ON DATABASE postgres TO pt_scram_user;
