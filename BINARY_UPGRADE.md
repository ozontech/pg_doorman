## Binary upgrade

When you send a SIGINT signal to the pg_doorman, the binary update process starts.
The old pg_doorman instance executes the `exec` command and starts a new daemonized process.
This new process works with the SO_REUSE_PORT parameter, and the operating system send traffic to new instance.
After that, the old instance closes the socket for incoming clients. 

Then we give the option to complete all current queries and transactions within shutdown_timeout (10s). 
After successful completion query/transaction for each new queries in session, we return an error with the code `58006`,
which means that the client needs to reconnect and after that, and client can safely repeat query.

### OffTopic:

Repeating query (without code `58006`) may cause problems described [here](https://github.com/lib/pq/issues/939)
General recommendation: be careful when using `github.com/lib/pq` or `database/sql`!