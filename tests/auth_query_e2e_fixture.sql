-- Fixtures for auth_query end-to-end tests
-- Creates auth_users table and populates it with test data

CREATE TABLE IF NOT EXISTS auth_users (
    username TEXT NOT NULL,
    password TEXT
);

-- Dynamic user with MD5 password hash (password = "secret1")
INSERT INTO auth_users VALUES ('auth_user1', 'md5' || md5('secret1' || 'auth_user1'));

-- Second dynamic user (password = "test123")
INSERT INTO auth_users VALUES ('auth_user2', 'md5' || md5('test123' || 'auth_user2'));

-- User for password rotation test (initial password = "old_pass")
INSERT INTO auth_users VALUES ('rotate_user', 'md5' || md5('old_pass' || 'rotate_user'));

-- User that also exists as static user (password = "dynamic_pass")
-- Static user should take priority over auth_query
INSERT INTO auth_users VALUES ('static_user', 'md5' || md5('dynamic_pass' || 'static_user'));
