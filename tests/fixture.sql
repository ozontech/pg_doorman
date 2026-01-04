create database example_db;

\c example_db;

--
set password_encryption to md5;
create user example_user_1 with password 'test';
alter user example_user_1 with superuser;

create user example_user_rollback with password 'test';
alter user example_user_rollback with superuser;

create user example_user_nopassword;
alter user example_user_nopassword with superuser;

create user example_user_disconnect;
alter user example_user_disconnect with superuser;

create user example_user_prometheus;
alter user example_user_prometheus with superuser;

create user example_user_auth_md5 with password 'test';
alter user example_user_auth_md5 with superuser;

create user example_user_jwt with password 'test';
alter user example_user_jwt with superuser;
--
set password_encryption to "scram-sha-256";
create user example_user_2 with password 'test';

alter system set log_min_duration_statement to 0;
alter system set log_line_prefix to '%m [%p] %q%u@%d/%a ';
alter system set log_connections to on;
alter system set log_disconnections to on;
alter system set log_min_messages to debug1;

select pg_reload_conf();
-- unix socket.
-- alter system set unix_socket_directories to '/tmp';