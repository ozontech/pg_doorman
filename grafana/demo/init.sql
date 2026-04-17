SET password_encryption = 'md5';
CREATE USER app_user WITH PASSWORD 'app_pass' SUPERUSER;
CREATE USER app_user_2 WITH PASSWORD 'app_pass_2' SUPERUSER;
CREATE USER doorman_auth WITH PASSWORD 'auth_secret';
CREATE DATABASE app_db OWNER app_user;

-- Auth query function
\c app_db
CREATE OR REPLACE FUNCTION pg_doorman_auth(p_username TEXT)
RETURNS TABLE(username TEXT, password TEXT) AS $$
BEGIN
    RETURN QUERY SELECT usename::TEXT, passwd::TEXT FROM pg_shadow WHERE usename = p_username;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;
GRANT EXECUTE ON FUNCTION pg_doorman_auth(TEXT) TO doorman_auth;
