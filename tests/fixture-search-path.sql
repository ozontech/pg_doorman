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

-- public.t is the role-default-`search_path` witness for the sticky
-- scenario. The PIN client pins `search_path=schema_a`; after PIN
-- disconnects, the PLAIN client connects without `search_path` so
-- the backend's `search_path` must reset to the role default, which
-- resolves the unqualified `t` against `public.t` (val=3), not the
-- previous client's `schema_a.t` (val=1).
create table if not exists public.t (val int);
delete from public.t;
insert into public.t values (3);

grant usage on schema schema_a to example_user_1;
grant usage on schema schema_b to example_user_1;
grant select on schema_a.t to example_user_1;
grant select on schema_b.t to example_user_1;
grant select on public.t to example_user_1;
