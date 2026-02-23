-- Fixtures for static user passthrough mode tests
-- Creates real PG users with MD5 and SCRAM passwords

-- MD5 user
SET password_encryption = 'md5';
CREATE USER pt_static_md5 WITH PASSWORD 'md5pass';
GRANT ALL ON DATABASE postgres TO pt_static_md5;

-- SCRAM user
SET password_encryption = 'scram-sha-256';
CREATE USER pt_static_scram WITH PASSWORD 'scrampass';
GRANT ALL ON DATABASE postgres TO pt_static_scram;
