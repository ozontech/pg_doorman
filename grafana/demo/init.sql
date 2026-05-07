SET password_encryption = 'md5';
CREATE USER app_user WITH PASSWORD 'app_pass' SUPERUSER;
CREATE USER app_user_2 WITH PASSWORD 'app_pass_2' SUPERUSER;
CREATE USER doorman_auth WITH PASSWORD 'auth_secret';
-- Session-mode demo user. Owns no objects; only LISTEN/NOTIFY rights.
CREATE USER app_session WITH PASSWORD 'session_pass';
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

-- Notification table + trigger for the session-mode listener sidecar.
-- LISTEN/NOTIFY requires session pool mode because the notification arrives
-- outside any transaction, and a transaction-pooled backend would have been
-- handed off to another client by then.
CREATE TABLE notify_queue (
    id SERIAL PRIMARY KEY,
    payload TEXT NOT NULL,
    ts TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE FUNCTION notify_queue_event() RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('app_events', NEW.payload);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER notify_queue_t
    AFTER INSERT ON notify_queue
    FOR EACH ROW EXECUTE FUNCTION notify_queue_event();

GRANT INSERT, SELECT ON notify_queue TO app_session, app_user_2;
GRANT USAGE, SELECT ON SEQUENCE notify_queue_id_seq TO app_session, app_user_2;

-- app_session runs read-only pgbench against the tables that app_user
-- creates with `pgbench -i`. We use both ALTER DEFAULT PRIVILEGES (covers
-- future objects) and an explicit GRANT (covers any tables that already
-- exist before the bench user is created — the order between init.sql
-- and pgbench.sh is timing-dependent on a cold start).
ALTER DEFAULT PRIVILEGES FOR USER app_user IN SCHEMA public
    GRANT SELECT ON TABLES TO app_session;

-- pgbench's startup vacuum is unconditional unless --no-vacuum is set,
-- and it touches all four pgbench tables; granting ALL keeps the
-- session-mode load running even if a future pgbench flavour issues
-- writes against the history table during startup. The bench logic
-- itself stays SELECT-only.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 'public' AND tablename = 'pgbench_accounts') THEN
        EXECUTE 'GRANT ALL ON pgbench_accounts, pgbench_branches, pgbench_history, pgbench_tellers TO app_session';
    END IF;
END $$;
