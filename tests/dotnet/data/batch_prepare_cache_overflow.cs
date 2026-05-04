using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

// Reproduces production "prepared statement DOORMAN_N does not exist"
// (SQLSTATE 26000) with the npgsql Max Auto Prepare flow:
// 1. Connection string enables Max Auto Prepare so npgsql implicitly
//    prepares any query that has been used Auto Prepare Min Usages times.
// 2. We warm up a pool of 500 distinct queries by running each one a few
//    times so npgsql auto-prepares them all.
// 3. We then issue an NpgsqlBatch of 500 of those (already-prepared)
//    commands at once, against pg_doorman with prepared_statements_cache_size
//    much smaller than the batch — so the per-connection server LRU has to
//    evict statements that are still referenced by Binds queued in the same
//    batch.

string baseConnectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

string connectionString = baseConnectionString
    + ";Max Auto Prepare=600;Auto Prepare Min Usages=2";

const int ClientCount = 8;
const int Iterations = 10;
const int BatchSize = 500;

Console.WriteLine($"batch_prepare_cache_overflow: starting "
    + $"(clients={ClientCount}, iters={Iterations}, batch={BatchSize})");

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
            startBarrier.SignalAndWait();

            // Warm up: run each query twice as a regular NpgsqlCommand so
            // npgsql triggers Auto Prepare and assigns a stable named
            // statement to it.
            for (int warm = 0; warm < 2; warm++)
            {
                for (int i = 0; i < BatchSize; i++)
                {
                    await using var cmd = new NpgsqlCommand(
                        $"select $1::int as q_{id}_{i}", connection);
                    cmd.Parameters.Add(new NpgsqlParameter
                    {
                        Value = i,
                        NpgsqlDbType = NpgsqlDbType.Integer,
                    });
                    var got = (int)(await cmd.ExecuteScalarAsync())!;
                    if (got != i)
                    {
                        throw new Exception(
                            $"client {id} warm {warm} q_{id}_{i}: got {got}");
                    }
                }
            }

            // Now run the full batch repeatedly. Every command in the batch
            // is auto-prepared (Min Usages = 2 hit during warm up), so the
            // batch is sent as Bind + Execute pairs without per-command Parse.
            for (int iter = 0; iter < Iterations; iter++)
            {
                await using var batch = connection.CreateBatch();
                for (int i = 0; i < BatchSize; i++)
                {
                    batch.BatchCommands.Add(new NpgsqlBatchCommand(
                        $"select $1::int as q_{id}_{i}")
                    {
                        Parameters =
                        {
                            new NpgsqlParameter
                            {
                                Value = i,
                                NpgsqlDbType = NpgsqlDbType.Integer,
                            }
                        }
                    });
                }

                await using var reader = await batch.ExecuteReaderAsync();
                int idx = 0;
                do
                {
                    if (!await reader.ReadAsync())
                    {
                        throw new Exception(
                            $"client {id} iter {iter} batch[{idx}]: missing row");
                    }
                    int got = reader.GetInt32(0);
                    if (got != idx)
                    {
                        throw new Exception(
                            $"client {id} iter {iter} batch[{idx}]: "
                            + $"expected {idx}, got {got}");
                    }
                    idx++;
                } while (await reader.NextResultAsync());

                if (idx != BatchSize)
                {
                    throw new Exception(
                        $"client {id} iter {iter}: expected {BatchSize} results, "
                        + $"got {idx}");
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
