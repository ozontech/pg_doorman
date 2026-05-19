-- Fixture for the @prepared-cache-startup-parameters BDD scenario.
-- Two schemas hold a table named `t` whose `val` column differs, so a
-- client that points `search_path` at the wrong schema receives a
-- visibly-wrong row instead of a more subtle planner mismatch.

\c example_db;

create schema if not exists schema_a;
create schema if not exists schema_b;

create table if not exists schema_a.t (val int);
delete from schema_a.t;
insert into schema_a.t values (1);

create table if not exists schema_b.t (val int);
delete from schema_b.t;
insert into schema_b.t values (2);

grant usage on schema schema_a to example_user_1;
grant usage on schema schema_b to example_user_1;
grant select on schema_a.t to example_user_1;
grant select on schema_b.t to example_user_1;
