create database example_db;

alter system set log_min_duration_statement to 0;
alter system set log_line_prefix to '%m [%p] %q%u@%d/%a ';
select pg_reload_conf();

\c example_db;

--
set password_encryption to md5;
create user example_user_1 with password 'test';
alter user example_user_1 with superuser;

create user example_user_auth_md5 with password 'test';
alter user example_user_auth_md5 with superuser;

create user example_user_jwt with password 'test';
alter user example_user_jwt with superuser;
--
set password_encryption to "scram-sha-256";
create user example_user_2 with password 'test';

-- unix socket.
-- alter system set unix_socket_directories to '/tmp';