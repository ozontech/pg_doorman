-- Fixtures for auth_query HBA integration tests
-- Creates auth_users table and populates with users for HBA edge-case testing

CREATE TABLE IF NOT EXISTS auth_users (
    username TEXT NOT NULL,
    password TEXT
);

-- MD5 user (password = "hba_md5_pass")
INSERT INTO auth_users VALUES ('hba_md5_user', 'md5' || md5('hba_md5_pass' || 'hba_md5_user'));

-- SCRAM user (password = "hba_scram_pass") — need real PG user for SCRAM verifier
SET password_encryption = 'scram-sha-256';
CREATE USER hba_scram_user WITH PASSWORD 'hba_scram_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'hba_scram_user';

-- Trust user — exists in auth_users with MD5 hash (password = "trust_pass")
SET password_encryption = 'md5';
INSERT INTO auth_users VALUES ('hba_trust_user', 'md5' || md5('trust_pass' || 'hba_trust_user'));

-- Passthrough trust user — real PG user for passthrough + trust HBA
CREATE USER hba_pt_trust_user WITH PASSWORD 'pt_trust_pass';
INSERT INTO auth_users
    SELECT rolname, rolpassword FROM pg_authid WHERE rolname = 'hba_pt_trust_user';

-- Grant database access
GRANT ALL ON DATABASE postgres TO hba_scram_user;
GRANT ALL ON DATABASE postgres TO hba_pt_trust_user;
