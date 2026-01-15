using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Data;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

// Aggressive mixed tests: batch + prepared statements + extended protocol
// These tests are intentionally aggressive and may expose server issues
// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Batch with prepared statements interleaved
Console.WriteLine("Test 1: Batch with prepared statements interleaved");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Setup table
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_aggressive_mixed;
        CREATE TABLE test_aggressive_mixed(
            id serial primary key, 
            value int,
            data text
        )", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int round = 0; round < 20; round++)
    {
        // First: batch insert
        await using var batch = new NpgsqlBatch(connection)
        {
            BatchCommands =
            {
                new($"INSERT INTO test_aggressive_mixed(value, data) VALUES ({round * 10 + 1}, 'batch1')"),
                new($"INSERT INTO test_aggressive_mixed(value, data) VALUES ({round * 10 + 2}, 'batch2')"),
                new($"INSERT INTO test_aggressive_mixed(value, data) VALUES ({round * 10 + 3}, 'batch3')"),
            }
        };
        await batch.ExecuteNonQueryAsync();

        // Immediately: prepared statement select
        using (var prepared = new NpgsqlCommand("SELECT * FROM test_aggressive_mixed WHERE value > @v ORDER BY id", connection))
        {
            prepared.Parameters.Add("v", NpgsqlDbType.Integer);
            prepared.Prepare();
            prepared.Parameters["v"].Value = round * 10;
            
            int count = 0;
            using (var reader = prepared.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 3) throw new Exception($"Round {round}: Expected 3, got {count}");
        }

        // Another batch with select
        await using var batch2 = new NpgsqlBatch(connection)
        {
            BatchCommands =
            {
                new($"SELECT * FROM test_aggressive_mixed WHERE value = {round * 10 + 1}"),
                new($"SELECT * FROM test_aggressive_mixed WHERE value = {round * 10 + 2}"),
                new($"SELECT * FROM test_aggressive_mixed WHERE value = {round * 10 + 3}"),
            }
        };
        
        int resultCount = 0;
        await using (var reader = await batch2.ExecuteReaderAsync())
        {
            do
            {
                while (await reader.ReadAsync()) resultCount++;
            } while (await reader.NextResultAsync());
        }
        if (resultCount != 3) throw new Exception($"Round {round} batch2: Expected 3 results, got {resultCount}");

        // Cleanup for next round
        using (var cleanup = new NpgsqlCommand("DELETE FROM test_aggressive_mixed", connection))
        {
            cleanup.ExecuteNonQuery();
        }
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Parallel batch and prepared statements from multiple connections
// AGGRESSIVE: INSERT and SELECT in same batch - SELECT may execute before INSERT completes
Console.WriteLine("Test 2: Parallel batch and prepared statements");
{
    var errors = new List<string>();
    var lockObj = new object();
    var barrier = new Barrier(12);
    var tasks = new List<Task>();

    // Setup shared table
    using (var setupConn = new NpgsqlConnection(connectionString))
    {
        setupConn.Open();
        using (var cmd = new NpgsqlCommand(@"
            DROP TABLE IF EXISTS test_parallel_mixed;
            CREATE TABLE test_parallel_mixed(
                id serial primary key,
                client_id int,
                round int,
                data text
            )", setupConn))
        {
            cmd.ExecuteNonQuery();
        }
    }

    for (int clientId = 0; clientId < 12; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();

                for (int round = 0; round < 15; round++)
                {
                    barrier.SignalAndWait();

                    if (id % 3 == 0)
                    {
                        // AGGRESSIVE: Batch with INSERT and SELECT in same batch
                        // This tests if server handles this correctly
                        await using var batch = new NpgsqlBatch(conn)
                        {
                            BatchCommands =
                            {
                                new($"INSERT INTO test_parallel_mixed(client_id, round, data) VALUES ({id}, {round}, 'batch_data')"),
                                new($"SELECT * FROM test_parallel_mixed WHERE client_id = {id} AND round = {round}"),
                            }
                        };
                        await using var reader = await batch.ExecuteReaderAsync();
                        // Use do-while pattern - Npgsql merges batch results
                        int totalRows = 0;
                        do
                        {
                            while (await reader.ReadAsync()) totalRows++;
                        } while (await reader.NextResultAsync());
                        if (totalRows != 1)
                        {
                            lock (lockObj) { errors.Add($"Client {id} round {round}: batch select expected 1 row, got {totalRows}"); }
                        }
                    }
                    else if (id % 3 == 1)
                    {
                        // Prepared statements
                        await using var insert = new NpgsqlCommand(
                            "INSERT INTO test_parallel_mixed(client_id, round, data) VALUES (@cid, @r, @d)", conn);
                        insert.Parameters.AddWithValue("cid", id);
                        insert.Parameters.AddWithValue("r", round);
                        insert.Parameters.AddWithValue("d", "prepared_data");
                        await insert.PrepareAsync();
                        await insert.ExecuteNonQueryAsync();

                        await using var select = new NpgsqlCommand(
                            "SELECT * FROM test_parallel_mixed WHERE client_id = @cid AND round = @r", conn);
                        select.Parameters.AddWithValue("cid", id);
                        select.Parameters.AddWithValue("r", round);
                        await select.PrepareAsync();
                        await using var reader = await select.ExecuteReaderAsync();
                        if (!await reader.ReadAsync())
                        {
                            lock (lockObj) { errors.Add($"Client {id} round {round}: prepared select returned no rows"); }
                        }
                    }
                    else
                    {
                        // Mixed: batch insert + prepared select
                        await using var batch = new NpgsqlBatch(conn)
                        {
                            BatchCommands =
                            {
                                new($"INSERT INTO test_parallel_mixed(client_id, round, data) VALUES ({id}, {round}, 'mixed_data')"),
                            }
                        };
                        await batch.ExecuteNonQueryAsync();

                        await using var select = new NpgsqlCommand(
                            "SELECT * FROM test_parallel_mixed WHERE client_id = @cid AND round = @r", conn);
                        select.Parameters.AddWithValue("cid", id);
                        select.Parameters.AddWithValue("r", round);
                        await select.PrepareAsync();
                        await using var reader = await select.ExecuteReaderAsync();
                        if (!await reader.ReadAsync())
                        {
                            lock (lockObj) { errors.Add($"Client {id} round {round}: mixed select returned no rows"); }
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                lock (lockObj)
                {
                    errors.Add($"Client {id}: {ex.GetType().Name}: {ex.Message}");
                }
            }
        }));
    }

    Task.WaitAll(tasks.ToArray());

    if (errors.Count > 0)
    {
        foreach (var err in errors) Console.WriteLine($"  ERROR: {err}");
        throw new Exception($"Parallel mixed test failed with {errors.Count} errors");
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Rapid batch/prepared switching on single connection
Console.WriteLine("Test 3: Rapid batch/prepared switching");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_rapid_switch;
        CREATE TABLE test_rapid_switch(id serial primary key, val int)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int i = 0; i < 100; i++)
    {
        if (i % 2 == 0)
        {
            // Batch
            await using var batch = new NpgsqlBatch(connection)
            {
                BatchCommands =
                {
                    new($"INSERT INTO test_rapid_switch(val) VALUES ({i})"),
                    new("SELECT COUNT(*) FROM test_rapid_switch"),
                }
            };
            await using var reader = await batch.ExecuteReaderAsync();
            // Use do-while pattern - read through all results to find COUNT value
            long count = 0;
            do
            {
                while (await reader.ReadAsync())
                {
                    // COUNT(*) returns bigint in first column
                    if (reader.FieldCount == 1 && !reader.IsDBNull(0))
                    {
                        count = reader.GetInt64(0);
                    }
                }
            } while (await reader.NextResultAsync());
            if (count != i + 1) throw new Exception($"Iter {i}: Expected {i + 1}, got {count}");
        }
        else
        {
            // Prepared
            using (var insert = new NpgsqlCommand("INSERT INTO test_rapid_switch(val) VALUES (@v)", connection))
            {
                insert.Parameters.AddWithValue("v", i);
                insert.Prepare();
                insert.ExecuteNonQuery();
            }
            using (var select = new NpgsqlCommand("SELECT COUNT(*) FROM test_rapid_switch", connection))
            {
                select.Prepare();
                long count = (long)select.ExecuteScalar()!;
                if (count != i + 1) throw new Exception($"Iter {i}: Expected {i + 1}, got {count}");
            }
        }
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Large batch with prepared statements in between
Console.WriteLine("Test 4: Large batch with prepared statements in between");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_large_batch;
        CREATE TABLE test_large_batch(id serial primary key, data text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int round = 0; round < 10; round++)
    {
        // Large batch with 20 commands
        var batch = new NpgsqlBatch(connection);
        for (int i = 0; i < 20; i++)
        {
            batch.BatchCommands.Add(new NpgsqlBatchCommand(
                $"INSERT INTO test_large_batch(data) VALUES ('{new string('X', 500)}')"));
        }
        await batch.ExecuteNonQueryAsync();
        await batch.DisposeAsync();

        // Prepared statement in between
        using (var prepared = new NpgsqlCommand("SELECT COUNT(*) FROM test_large_batch", connection))
        {
            prepared.Prepare();
            long count = (long)prepared.ExecuteScalar()!;
            if (count != (round + 1) * 20) throw new Exception($"Round {round}: Expected {(round + 1) * 20}, got {count}");
        }

        // Another large batch with selects
        var selectBatch = new NpgsqlBatch(connection);
        for (int i = 0; i < 10; i++)
        {
            selectBatch.BatchCommands.Add(new NpgsqlBatchCommand(
                $"SELECT * FROM test_large_batch WHERE id > {round * 20 + i * 2} LIMIT 5"));
        }
        
        int totalRows = 0;
        await using (var reader = await selectBatch.ExecuteReaderAsync())
        {
            do
            {
                while (await reader.ReadAsync()) totalRows++;
            } while (await reader.NextResultAsync());
        }
        await selectBatch.DisposeAsync();
        
        // Each of 10 selects should return up to 5 rows
        if (totalRows < 10) throw new Exception($"Round {round}: Expected at least 10 rows, got {totalRows}");
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Batch with parameters (extended protocol batch)
Console.WriteLine("Test 5: Batch with parameters");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_batch_params;
        CREATE TABLE test_batch_params(id serial primary key, a int, b text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int round = 0; round < 20; round++)
    {
        var batch = new NpgsqlBatch(connection);
        
        // Add parameterized commands
        for (int i = 0; i < 5; i++)
        {
            var cmd = new NpgsqlBatchCommand("INSERT INTO test_batch_params(a, b) VALUES ($1, $2)");
            cmd.Parameters.AddWithValue(round * 5 + i);
            cmd.Parameters.AddWithValue($"data_{round}_{i}");
            batch.BatchCommands.Add(cmd);
        }
        
        // Add parameterized select
        var selectCmd = new NpgsqlBatchCommand("SELECT * FROM test_batch_params WHERE a >= $1 AND a < $2");
        selectCmd.Parameters.AddWithValue(round * 5);
        selectCmd.Parameters.AddWithValue((round + 1) * 5);
        batch.BatchCommands.Add(selectCmd);

        int selectCount = 0;
        await using (var reader = await batch.ExecuteReaderAsync())
        {
            // Use do-while pattern - Npgsql merges batch results
            do
            {
                while (await reader.ReadAsync()) selectCount++;
            } while (await reader.NextResultAsync());
        }
        await batch.DisposeAsync();

        if (selectCount != 5) throw new Exception($"Round {round}: Expected 5, got {selectCount}");

        // Verify with prepared statement
        using (var verify = new NpgsqlCommand("SELECT COUNT(*) FROM test_batch_params WHERE a >= @min AND a < @max", connection))
        {
            verify.Parameters.AddWithValue("min", round * 5);
            verify.Parameters.AddWithValue("max", (round + 1) * 5);
            verify.Prepare();
            long count = (long)verify.ExecuteScalar()!;
            if (count != 5) throw new Exception($"Round {round} verify: Expected 5, got {count}");
        }
    }
}
Console.WriteLine("Test 5 complete");

// Test 6: Concurrent batch operations with transaction isolation
Console.WriteLine("Test 6: Concurrent batch with transactions");
{
    var errors = new List<string>();
    var lockObj = new object();
    var tasks = new List<Task>();

    using (var setupConn = new NpgsqlConnection(connectionString))
    {
        setupConn.Open();
        using (var cmd = new NpgsqlCommand(@"
            DROP TABLE IF EXISTS test_batch_tx;
            CREATE TABLE test_batch_tx(id serial primary key, client_id int, value int)", setupConn))
        {
            cmd.ExecuteNonQuery();
        }
    }

    for (int clientId = 0; clientId < 8; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();

                for (int round = 0; round < 10; round++)
                {
                    await using var tx = await conn.BeginTransactionAsync();

                    // Batch insert within transaction
                    var batch = new NpgsqlBatch(conn);
                    for (int i = 0; i < 5; i++)
                    {
                        batch.BatchCommands.Add(new NpgsqlBatchCommand(
                            $"INSERT INTO test_batch_tx(client_id, value) VALUES ({id}, {round * 5 + i})"));
                    }
                    await batch.ExecuteNonQueryAsync();
                    await batch.DisposeAsync();

                    // Prepared select within same transaction
                    await using var select = new NpgsqlCommand(
                        "SELECT COUNT(*) FROM test_batch_tx WHERE client_id = @cid", conn);
                    select.Parameters.AddWithValue("cid", id);
                    await select.PrepareAsync();
                    long count = (long)(await select.ExecuteScalarAsync())!;
                    
                    long expected = (round + 1) * 5;
                    if (count != expected)
                    {
                        lock (lockObj) { errors.Add($"Client {id} round {round}: Expected {expected}, got {count}"); }
                    }

                    await tx.CommitAsync();
                }
            }
            catch (Exception ex)
            {
                lock (lockObj)
                {
                    errors.Add($"Client {id}: {ex.GetType().Name}: {ex.Message}");
                }
            }
        }));
    }

    Task.WaitAll(tasks.ToArray());

    if (errors.Count > 0)
    {
        foreach (var err in errors) Console.WriteLine($"  ERROR: {err}");
        throw new Exception($"Concurrent batch tx test failed with {errors.Count} errors");
    }
}
Console.WriteLine("Test 6 complete");

// Test 7: Stress test - rapid fire batch + prepared + simple queries
Console.WriteLine("Test 7: Rapid fire mixed queries stress test");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_stress_mixed;
        CREATE TABLE test_stress_mixed(id serial primary key, t text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    var random = new Random(42);
    
    for (int i = 0; i < 200; i++)
    {
        int choice = random.Next(4);
        
        switch (choice)
        {
            case 0:
                // Simple query
                using (var simple = new NpgsqlCommand($"INSERT INTO test_stress_mixed(t) VALUES ('simple_{i}')", connection))
                {
                    simple.ExecuteNonQuery();
                }
                break;
                
            case 1:
                // Prepared statement
                using (var prepared = new NpgsqlCommand("INSERT INTO test_stress_mixed(t) VALUES (@t)", connection))
                {
                    prepared.Parameters.AddWithValue("t", $"prepared_{i}");
                    prepared.Prepare();
                    prepared.ExecuteNonQuery();
                }
                break;
                
            case 2:
                // Batch insert
                await using (var batch = new NpgsqlBatch(connection)
                {
                    BatchCommands =
                    {
                        new($"INSERT INTO test_stress_mixed(t) VALUES ('batch_{i}_a')"),
                        new($"INSERT INTO test_stress_mixed(t) VALUES ('batch_{i}_b')"),
                    }
                })
                {
                    await batch.ExecuteNonQueryAsync();
                }
                break;
                
            case 3:
                // Batch with parameters
                await using (var paramBatch = new NpgsqlBatch(connection))
                {
                    var cmd1 = new NpgsqlBatchCommand("INSERT INTO test_stress_mixed(t) VALUES ($1)");
                    cmd1.Parameters.AddWithValue($"param_batch_{i}");
                    paramBatch.BatchCommands.Add(cmd1);
                    await paramBatch.ExecuteNonQueryAsync();
                }
                break;
        }
        
        // Periodically verify count
        if (i % 50 == 49)
        {
            using (var count = new NpgsqlCommand("SELECT COUNT(*) FROM test_stress_mixed", connection))
            {
                count.Prepare();
                long c = (long)count.ExecuteScalar()!;
                if (c < i) throw new Exception($"Iter {i}: Count {c} is less than expected minimum {i}");
            }
        }
    }
}
Console.WriteLine("Test 7 complete");

// Test 8: Batch with errors - partial failure handling
Console.WriteLine("Test 8: Batch with errors handling");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_batch_errors;
        CREATE TABLE test_batch_errors(id serial primary key, val int UNIQUE)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int round = 0; round < 10; round++)
    {
        // First batch - should succeed
        await using (var batch1 = new NpgsqlBatch(connection)
        {
            BatchCommands =
            {
                new($"INSERT INTO test_batch_errors(val) VALUES ({round * 100 + 1})"),
                new($"INSERT INTO test_batch_errors(val) VALUES ({round * 100 + 2})"),
            }
        })
        {
            await batch1.ExecuteNonQueryAsync();
        }

        // Prepared statement after batch
        using (var prepared = new NpgsqlCommand("SELECT COUNT(*) FROM test_batch_errors", connection))
        {
            prepared.Prepare();
            long count = (long)prepared.ExecuteScalar()!;
            if (count != (round + 1) * 2) throw new Exception($"Round {round}: Expected {(round + 1) * 2}, got {count}");
        }

        // Batch that will fail (duplicate key) - wrapped in try-catch
        try
        {
            await using var batch2 = new NpgsqlBatch(connection)
            {
                BatchCommands =
                {
                    new($"INSERT INTO test_batch_errors(val) VALUES ({round * 100 + 1})"), // duplicate!
                }
            };
            await batch2.ExecuteNonQueryAsync();
            throw new Exception($"Round {round}: Expected duplicate key error");
        }
        catch (PostgresException ex) when (ex.SqlState == "23505")
        {
            // Expected - unique violation
        }

        // Connection should still work after error
        using (var verify = new NpgsqlCommand("SELECT COUNT(*) FROM test_batch_errors", connection))
        {
            verify.Prepare();
            long count = (long)verify.ExecuteScalar()!;
            if (count != (round + 1) * 2) throw new Exception($"Round {round} after error: Expected {(round + 1) * 2}, got {count}");
        }
    }
}
Console.WriteLine("Test 8 complete");

// Test 9: Extended protocol with Describe interleaved with batch
Console.WriteLine("Test 9: Extended protocol Describe with batch");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_describe_batch;
        CREATE TABLE test_describe_batch(
            id serial primary key, 
            col1 text, col2 int, col3 boolean, col4 timestamp
        )", connection))
    {
        cmd.ExecuteNonQuery();
    }

    for (int round = 0; round < 15; round++)
    {
        // Batch insert
        await using (var batch = new NpgsqlBatch(connection)
        {
            BatchCommands =
            {
                new($"INSERT INTO test_describe_batch(col1, col2, col3, col4) VALUES ('test', {round}, true, NOW())"),
            }
        })
        {
            await batch.ExecuteNonQueryAsync();
        }

        // SchemaOnly query (triggers Describe)
        using (var describe = new NpgsqlCommand("SELECT * FROM test_describe_batch", connection))
        {
            using (var reader = describe.ExecuteReader(CommandBehavior.SchemaOnly))
            {
                var schema = reader.GetSchemaTable();
                if (schema == null || schema.Rows.Count != 5)
                    throw new Exception($"Round {round}: Expected 5 columns in schema");
            }
        }

        // Prepared with explicit Prepare (also triggers Describe)
        using (var prepared = new NpgsqlCommand("SELECT col1, col2 FROM test_describe_batch WHERE id = @id", connection))
        {
            prepared.Parameters.Add("id", NpgsqlDbType.Integer);
            prepared.Prepare();
            prepared.Parameters["id"].Value = round + 1;
            
            using (var reader = prepared.ExecuteReader())
            {
                if (!reader.Read()) throw new Exception($"Round {round}: No row found");
                if (reader.FieldCount != 2) throw new Exception($"Round {round}: Expected 2 columns, got {reader.FieldCount}");
            }
        }

        // Batch select
        await using (var selectBatch = new NpgsqlBatch(connection)
        {
            BatchCommands =
            {
                new("SELECT COUNT(*) FROM test_describe_batch"),
                new($"SELECT * FROM test_describe_batch WHERE id = {round + 1}"),
            }
        })
        {
            await using var reader = await selectBatch.ExecuteReaderAsync();
            await reader.ReadAsync();
            long count = reader.GetInt64(0);
            if (count != round + 1) throw new Exception($"Round {round}: Expected count {round + 1}, got {count}");
        }
    }
}
Console.WriteLine("Test 9 complete");

// Test 10: Maximum stress - all patterns combined with high concurrency
Console.WriteLine("Test 10: Maximum stress combined patterns");
{
    var errors = new List<string>();
    var lockObj = new object();
    var tasks = new List<Task>();
    var barrier = new Barrier(20);

    using (var setupConn = new NpgsqlConnection(connectionString))
    {
        setupConn.Open();
        using (var cmd = new NpgsqlCommand(@"
            DROP TABLE IF EXISTS test_max_stress;
            CREATE TABLE test_max_stress(
                id serial primary key, 
                client_id int, 
                round int, 
                pattern text,
                data text
            )", setupConn))
        {
            cmd.ExecuteNonQuery();
        }
    }

    for (int clientId = 0; clientId < 20; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();

                for (int round = 0; round < 20; round++)
                {
                    barrier.SignalAndWait();

                    // Pattern based on client and round
                    int pattern = (id + round) % 5;

                    switch (pattern)
                    {
                        case 0:
                            // AGGRESSIVE: Simple batch with INSERT+SELECT in same batch
                            await using (var batch = new NpgsqlBatch(conn)
                            {
                                BatchCommands =
                                {
                                    new($"INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES ({id}, {round}, 'batch', 'data')"),
                                    new($"SELECT * FROM test_max_stress WHERE client_id = {id} AND round = {round}"),
                                }
                            })
                            {
                                await using var reader = await batch.ExecuteReaderAsync();
                                // Use do-while pattern - Npgsql merges batch results
                                int totalRows = 0;
                                do
                                {
                                    while (await reader.ReadAsync()) totalRows++;
                                } while (await reader.NextResultAsync());
                                if (totalRows != 1)
                                {
                                    lock (lockObj) { errors.Add($"Client {id} round {round} pattern 0: expected 1 row, got {totalRows}"); }
                                }
                            }
                            break;

                        case 1:
                            // Prepared only
                            await using (var insert = new NpgsqlCommand(
                                "INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (@c, @r, @p, @d)", conn))
                            {
                                insert.Parameters.AddWithValue("c", id);
                                insert.Parameters.AddWithValue("r", round);
                                insert.Parameters.AddWithValue("p", "prepared");
                                insert.Parameters.AddWithValue("d", new string('P', 100));
                                await insert.PrepareAsync();
                                await insert.ExecuteNonQueryAsync();
                            }
                            break;

                        case 2:
                            // Batch with parameters
                            await using (var paramBatch = new NpgsqlBatch(conn))
                            {
                                var cmd = new NpgsqlBatchCommand(
                                    "INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES ($1, $2, $3, $4)");
                                cmd.Parameters.AddWithValue(id);
                                cmd.Parameters.AddWithValue(round);
                                cmd.Parameters.AddWithValue("param_batch");
                                cmd.Parameters.AddWithValue(new string('B', 100));
                                paramBatch.BatchCommands.Add(cmd);
                                await paramBatch.ExecuteNonQueryAsync();
                            }
                            break;

                        case 3:
                            // Mixed: batch insert + prepared select
                            await using (var batch = new NpgsqlBatch(conn)
                            {
                                BatchCommands =
                                {
                                    new($"INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES ({id}, {round}, 'mixed', 'data')"),
                                }
                            })
                            {
                                await batch.ExecuteNonQueryAsync();
                            }
                            await using (var select = new NpgsqlCommand(
                                "SELECT * FROM test_max_stress WHERE client_id = @c AND round = @r", conn))
                            {
                                select.Parameters.AddWithValue("c", id);
                                select.Parameters.AddWithValue("r", round);
                                await select.PrepareAsync();
                                await using var reader = await select.ExecuteReaderAsync();
                                if (!await reader.ReadAsync())
                                {
                                    lock (lockObj) { errors.Add($"Client {id} round {round} pattern 3: no row"); }
                                }
                            }
                            break;

                        case 4:
                            // Transaction with batch and prepared
                            await using (var tx = await conn.BeginTransactionAsync())
                            {
                                await using (var batch = new NpgsqlBatch(conn)
                                {
                                    BatchCommands =
                                    {
                                        new($"INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES ({id}, {round}, 'tx_batch', 'data')"),
                                    }
                                })
                                {
                                    await batch.ExecuteNonQueryAsync();
                                }
                                await using (var verify = new NpgsqlCommand(
                                    "SELECT COUNT(*) FROM test_max_stress WHERE client_id = @c", conn))
                                {
                                    verify.Parameters.AddWithValue("c", id);
                                    await verify.PrepareAsync();
                                    long count = (long)(await verify.ExecuteScalarAsync())!;
                                    if (count < 1)
                                    {
                                        lock (lockObj) { errors.Add($"Client {id} round {round} pattern 4: count {count}"); }
                                    }
                                }
                                await tx.CommitAsync();
                            }
                            break;
                    }
                }
            }
            catch (Exception ex)
            {
                lock (lockObj)
                {
                    errors.Add($"Client {id}: {ex.GetType().Name}: {ex.Message}\n{ex.StackTrace}");
                }
            }
        }));
    }

    Task.WaitAll(tasks.ToArray());

    if (errors.Count > 0)
    {
        foreach (var err in errors) Console.WriteLine($"  ERROR: {err}");
        throw new Exception($"Maximum stress test failed with {errors.Count} errors");
    }

    // Final verification
    using (var verifyConn = new NpgsqlConnection(connectionString))
    {
        verifyConn.Open();
        using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_max_stress", verifyConn))
        {
            long total = (long)cmd.ExecuteScalar()!;
            // 20 clients * 20 rounds = 400 rows expected
            if (total != 400) throw new Exception($"Final count: Expected 400, got {total}");
        }
    }
}
Console.WriteLine("Test 10 complete");

Console.WriteLine("aggressive_mixed complete");
