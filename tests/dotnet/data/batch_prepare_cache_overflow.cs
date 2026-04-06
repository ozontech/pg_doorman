using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

// Regression test for "prepared statement DOORMAN_N does not exist" (SQLSTATE
// 26000) under concurrent npgsql batch prepare workloads. Several connections
// share a tiny pool of distinct queries so they contend for the same DOORMAN_N
// entries in pg_doorman's pool cache and constantly evict each other from the
// per-connection server LRU. With prepared_statements_cache_size=1 every
// PrepareAsync triggers an eviction.

string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

const int ClientCount = 16;
const int StatementsPerClient = 10;
const int Iterations = 50;
const int SharedQueryPool = 32;

Console.WriteLine($"batch_prepare_cache_overflow: starting "
    + $"(clients={ClientCount}, stmts/client={StatementsPerClient}, iters={Iterations}, "
    + $"shared_pool={SharedQueryPool})");

var errors = new List<string>();
var errorsLock = new object();
var startBarrier = new Barrier(ClientCount);
var tasks = new List<Task>();

for (int clientId = 0; clientId < ClientCount; clientId++)
{
    int id = clientId;
    tasks.Add(Task.Run(async () =>
    {
        try
        {
            await using var connection = new NpgsqlConnection(connectionString);
            await connection.OpenAsync();

            // Synchronise the start so all clients hit pg_doorman together.
            startBarrier.SignalAndWait();

            for (int iter = 0; iter < Iterations; iter++)
            {
                int offset = (id + iter) % SharedQueryPool;

                await using var batch = connection.CreateBatch();
                for (int i = 0; i < StatementsPerClient; i++)
                {
                    int queryIndex = (offset + i) % SharedQueryPool;
                    batch.BatchCommands.Add(new NpgsqlBatchCommand(
                        $"select $1::int as shared_q_{queryIndex}")
                    {
                        Parameters =
                        {
                            new NpgsqlParameter
                            {
                                Value = queryIndex,
                                NpgsqlDbType = NpgsqlDbType.Integer,
                            }
                        }
                    });
                }

                await batch.PrepareAsync();
                await using var reader = await batch.ExecuteReaderAsync();

                int cmdIndex = 0;
                do
                {
                    if (!await reader.ReadAsync())
                    {
                        throw new Exception(
                            $"client {id} iter {iter} cmd {cmdIndex}: missing row");
                    }
                    int got = reader.GetInt32(0);
                    int expected = (offset + cmdIndex) % SharedQueryPool;
                    if (got != expected)
                    {
                        throw new Exception(
                            $"client {id} iter {iter} cmd {cmdIndex}: "
                            + $"expected {expected}, got {got}");
                    }
                    cmdIndex++;
                } while (await reader.NextResultAsync());

                if (cmdIndex != StatementsPerClient)
                {
                    throw new Exception(
                        $"client {id} iter {iter}: expected {StatementsPerClient} "
                        + $"results, got {cmdIndex}");
                }
            }
        }
        catch (Exception ex)
        {
            lock (errorsLock)
            {
                errors.Add($"client {id}: {ex.GetType().Name}: {ex.Message}");
            }
        }
    }));
}

await Task.WhenAll(tasks);

if (errors.Count > 0)
{
    foreach (var err in errors)
    {
        Console.WriteLine($"  ERROR: {err}");
    }
    throw new Exception(
        $"batch_prepare_cache_overflow failed with {errors.Count} client error(s)");
}

Console.WriteLine("batch_prepare_cache_overflow complete");
