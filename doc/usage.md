# pg_doorman


## Synopsis

    pg_doorman [OPTIONS] [CONFIG_FILE]

    Options:
      -l, --log-level <LOG_LEVEL>    [env: LOG_LEVEL=] [default: INFO]
      -F, --log-format <LOG_FORMAT>  [env: LOG_FORMAT=] [default: text] [possible values: text, structured, debug]
      -n, --no-color                 disable colors in the log output [env: NO_COLOR=]
      -d, --daemon                   run as daemon [env: DAEMON=]
      -h, --help                     Print help
      -V, --version                  Print version

## Description

**pg_doorman** is a PostgreSQL connection pooler. Any target application
can be connected to **pg_doorman** as if it were a PostgreSQL server,
and **pg_doorman** will create a connection to the actual server, or it
will reuse one of its existing connections.

The aim of **pg_doorman** is to lower the performance impact of opening
new connections to PostgreSQL.

In order not to compromise transaction semantics for connection
pooling, **pg_doorman** supports several types of pooling when
rotating connections:

Session pooling
:   Most polite method. When a client connects, a server connection will
    be assigned to it for the whole duration the client stays connected. When
    the client disconnects, the server connection will be put back into the pool.
    This is the default method.

Transaction pooling
:   A server connection is assigned to a client only during a transaction.
    When PgDoorman notices that transaction is over, the server connection
    will be put back into the pool.

The administration interface of **pg_doorman** consists of some new
`SHOW` commands available when connected to a special "virtual"
database **pgbouncer** or **pgdoorman**.

## Quick-start

Basic setup and usage is as follows.

1. Create a pg_doorman.toml file.  Details in [config.md](config.md). Simple example:

    ```toml
        [general]
        host = "0.0.0.0"
        port = 6432
        
        admin_username = "admin"
        admin_password = "admin"
        
        [pools]
        
        [pools.exampledb]
        server_host = "127.0.0.1"
        server_port = 5432
        pool_mode = "transaction"
        
        [pools.exampledb.users.0]
        pool_size = 40
        username = "doorman"
        password = "SCRAM-SHA-256$4096:6nD+Ppi9rgaNyP7...MBiTld7xJipwG/X4="
    ```

2. Launch **pg_doorman**:

        $ pg_doorman pg_doorman.toml

3. Have your application (or the **psql** client) connect to
   **pg_doorman** instead of directly to the PostgreSQL server:

        $ psql -p 6432 -U doorman exampledb

4. Manage **pg_doorman** by connecting to the special administration
   database **pgdoorman** and issuing `SHOW HELP;` to begin:

        $ psql -p 6432 -U admin pgdoorman
			pgdoorman=> show help;
			NOTICE:  Console usage
			DETAIL:
				SHOW HELP|CONFIG|DATABASES|POOLS|POOLS_EXTENDED|CLIENTS|SERVERS|USERS|VERSION
				SHOW LISTS
				SHOW CONNECTIONS
				SHOW STATS
				RELOAD
				SHUTDOWN
			SHOW

5. If you made changes to the pg_doorman.toml file, you can reload it with:

        pgdoorman=# RELOAD;

## Command line switches

`-d`, `--daemon`
:   Run in the background. Without it, the process will run in the foreground.

    In daemon mode, setting `daemon_pid_file` as well as `syslog_prog_name`
    is required.  No log messages will be written to stderr after
    going into the background.

`-l`, `--log-level`
:   Set log-level: `INFO` or `DEBUG` or `WARN`.

`-F`, `--log-format`
:   Possible values: text, structured, debug.

`-n`, `--no-color`
:   Disable colors in the log output.

`-V`, `--version`
:   Show version.

`-h`, `--help`
:   Show short help.


## Admin console

The console is available by connecting as normal to the
database **pgdoorman** or **pgbouncer**:

    $ psql -p 6432 pgdoorman

Only user `admin_username` are allowed to log in to the console.

The admin console currently only supports the simple query protocol.
Some drivers use the extended query protocol for all commands; these
drivers will not work for this.

### Show commands

The **SHOW** commands output information. Each command is described below.

#### SHOW STATS

Shows statistics.  In this and related commands, the total figures are
since process start, the averages are updated every `15 seconds`.

database
:   Statistics are presented per database.

total_xact_count
:   Total number of SQL transactions pooled by **pg_doorman**.

total_query_count
:   Total number of SQL commands pooled by **pg_doorman**.

total_received
:   Total volume in bytes of network traffic received by **pg_doorman**.

total_sent
:   Total volume in bytes of network traffic sent by **pg_doorman**.

total_xact_time
:   Total number of microseconds spent by **pg_doorman** when connected
    to PostgreSQL in a transaction, either idle in transaction or
    executing queries.

total_query_time
:   Total number of microseconds spent by **pg_doorman** when actively
    connected to PostgreSQL, executing queries.

total_wait_time
:   Time spent by clients waiting for a server, in microseconds. Updated
    when a client connection is assigned a backend connection.

avg_xact_count
:   Average transactions per second in last stat period.

avg_query_count
:   Average queries per second in last stat period.

avg_server_assignment_count
:   Average number of times a server as assigned to a client per second in the
    last stat period.

avg_recv
:   Average received (from clients) bytes per second.

avg_sent
:   Average sent (to clients) bytes per second.

avg_xact_time
:   Average transaction duration, in microseconds.

avg_query_time
:   Average query duration, in microseconds.

avg_wait_time
:   Time spent by clients waiting for a server, in microseconds (average
    of the wait times for clients assigned a backend during the current
    `15 seconds`).

#### SHOW SERVERS

server_id
:   Unique ID for server.

server_process_id
:   PID of backend server process.  In case connection is made over
    Unix socket and OS supports getting process ID info, its
    OS PID.

database_name
:   Database name.

user
:   User name **pg_doorman** uses to connect to server.

application_name
:   A string containing the `application_name` set on the server connection.

state
:   State of the pg_doorman server connection, one of **active**,
    **idle**, **used**.

wait
:   Wait state of the pg_doorman server connection, one of **idle**,
    **read**, **write**.

transaction_count
:   Total number of processed transactions.

query_count
:   Total number of processed queries.

bytes_sent
:   Total bytes sent to PostgreSQL server.

bytes_received
:   Total bytes received from PostgreSQL server.

age_seconds
:   Lifetime of the current server connection.

prepare_cache_hit
:   Total number of cache hit prepared statements.

prepare_cache_miss
:   Total number of cache miss prepared statements.

prepare_cache_size
:   The total number of unique prepared statements.

#### SHOW CLIENTS

client_id
:   Unique ID for client.

database
:   Database (pool) name.

user
:   Client connected user.

addr
:   IP:port of client.

tls
:   Can be **true**, **false**.

state
:   State of the client connection, one of **active**,
    **idle**, **waiting**.

wait
:   Wait state of the pg_doorman client connection, one of **idle**,
    **read**, **write**.

transaction_count
:   Total number of processed transactions.

query_count
:   Total number of processed queries.

age_seconds
:   Lifetime of the current client connection.


#### SHOW POOLS

A new pool entry is made for each couple of (database, user).

database
:   Database name.

user
:   User name.

pool_mode
:   The pooling mode in use.

cl_active
:   Client connections that are either linked to server connections or are idle with no queries waiting to be processed.

cl_waiting
:   Client connections that have sent queries but have not yet got a server connection.

sv_active
:   Server connections that are linked to a client.

sv_idle
:   Server connections that are unused and immediately usable for client queries.

sv_login
:   Server connections currently in the process of logging in.

maxwait
:   How long the first (oldest) client in the queue has waited, in seconds.
    If this starts increasing, then the current pool of servers does
    not handle requests quickly enough.  The reason may be either an overloaded
    server or just too small of a **pool_size** setting.

maxwait_us
:   Microsecond part of the maximum waiting time.

#### SHOW USERS

name
:   The user name

pool_size
:   The user's override pool_size. or NULL if not set.

#### SHOW DATABASES

database
:   Name of configured database entry.

host
:   Host pg_doorman connects to.

port
:   Port pg_doorman connects to.

pool_size
:   Maximum number of server connections.

min_pool_size
:   Minimum number of server connections.

reserve_pool_size
:   Maximum number of additional connections for this database.

pool_mode
:   The database's override pool_mode, or NULL if the default will be used instead.

max_connections
:   Maximum number of allowed server connections for this database, as set by
    **max_db_connections**, either globally or per database.

current_connections
:   Current number of server connections for this database.

#### SHOW SOCKETS

Shows low-level information about sockets or only active sockets.
This includes the information shown under **SHOW CLIENTS** and **SHOW
SERVERS** as well as other more low-level information.


#### SHOW VERSION

Show the PgDoorman version string.

#### SHUTDOWN

The PgDoorman process will exit.

#### RELOAD

The PgDoorman process will reload its configuration files and update
changeable settings.

PgDoorman notices when a configuration file reload changes the
connection parameters of a database definition.  An existing server
connection to the old destination will be closed when the server
connection is next released (according to the pooling mode), and new
server connections will immediately use the updated connection
parameters.


### Signals

SIGHUP
:   Reload config. Same as issuing the command **RELOAD** on the console.

SIGTERM
:   Immediate shutdown.

SIGINT
:   Graceful shutdown [looks here](binary_upgrade.md) for more information.
