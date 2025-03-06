## Binary upgrade

When you send a SIGINT signal to the pg_doorman, the binary update process starts.
The old pg_doorman instance executes the exec command and starts a new process. This new process works with the SO_REUSE_PORT parameter, and the operating system send traffic to new instance.
After that, the old instance closes the socket for incoming clients. 

Then we give the option to complete all current queries and transactions within shutdown_timeout (10s). 
After successful completion query/transaction for each new queries in session, we return an error with the code `58006`,
which means that the client needs to reconnect and after that, client can safely repeat query.
Repeating the query without code 58006 may cause problems described [here](https://github.com/lib/pq/issues/939).

![binary upgrade](/images/binary-upgrade.png)