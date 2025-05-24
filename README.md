![pg_doorman](/static/logo_color_bg.png)

## PgDoorman: PostgreSQL Pooler

PgDoorman is a good alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), and [PgCat](https://github.com/postgresml/pgcat).
We aimed to create a more efficient, multithreaded version of PgBouncer
and with focus on performing pooler tasks efficiently and fast, in line with the Unix philosophy.
While we’ve removed load balancing and sharding, we believe it’s more efficient to handle these at the application level.
Over two years of use, we've improved driver support for languages like Go (pgx), .NET (npgsql), and asynchronous drivers for Python and Node.js.

For more information look at [Documentation](https://ozontech.github.io/pg_doorman/)