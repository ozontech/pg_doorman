-- Auth query test fixtures: table simulating pg_shadow for auth_query tests

CREATE TABLE auth_users (
    username TEXT NOT NULL,
    password TEXT
);

-- User with MD5 password hash
INSERT INTO auth_users VALUES ('test_user_md5', 'md53175bce1d3201d16594cebf9d7eb3f9d');

-- User with SCRAM password hash
INSERT INTO auth_users VALUES ('test_user_scram', 'SCRAM-SHA-256$4096:c2FsdA==$storedkey:serverkey');

-- User with NULL password (no auth possible)
INSERT INTO auth_users VALUES ('test_user_null_pw', NULL);

-- User with empty password (no auth possible)
INSERT INTO auth_users VALUES ('test_user_empty_pw', '');

-- Duplicate rows: query returns >1 row — executor must return error
INSERT INTO auth_users VALUES ('duplicate_user', 'md5hash1');
INSERT INTO auth_users VALUES ('duplicate_user', 'md5hash2');
