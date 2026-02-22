-- Fixtures for auth_query SCRAM e2e tests
-- Creates auth_users table and populates it with SCRAM verifiers from pg_authid

CREATE TABLE IF NOT EXISTS auth_users (
    username TEXT NOT NULL,
    password TEXT
);

-- Create PG users with SCRAM-SHA-256 passwords
SET password_encryption = 'scram-sha-256';

CREATE USER scram_aq_user WITH PASSWORD 'scram_secret';
CREATE USER scram_rotate_user WITH PASSWORD 'old_scram_pass';

-- Copy SCRAM verifiers from pg_authid into auth_users
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'scram_aq_user';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'scram_rotate_user';
