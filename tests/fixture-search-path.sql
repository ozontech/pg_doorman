-- Fixture for the @prepared-cache-startup-parameters BDD scenarios.
-- Each schema has a table named `t` with a different value, making
-- wrong search_path reuse visible as the wrong row.

\c example_db;

create schema if not exists schema_a;
create schema if not exists schema_b;

create table if not exists schema_a.t (val int);
delete from schema_a.t;
insert into schema_a.t values (1);

create table if not exists schema_b.t (val int);
delete from schema_b.t;
insert into schema_b.t values (2);

-- public.t proves the sticky case resets to the role-default
-- search_path after a client that pinned schema_a disconnects.
create table if not exists public.t (val int);
delete from public.t;
insert into public.t values (3);

grant usage on schema schema_a to example_user_1;
grant usage on schema schema_b to example_user_1;
grant select on schema_a.t to example_user_1;
grant select on schema_b.t to example_user_1;
grant select on public.t to example_user_1;
