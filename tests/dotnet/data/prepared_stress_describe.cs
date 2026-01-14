using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Data;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Rapid Describe requests with large results - stress test for ParseComplete insertion
// This test specifically targets the scenario where ParseComplete might be inserted
// between ParameterDescription and RowDescription messages
Console.WriteLine("Test 1: Rapid Describe with large results");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create table with very large data to ensure >8KB responses
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_stress_describe;
        CREATE TABLE test_stress_describe(
            id serial primary key, 
            col1 text, col2 text, col3 text, col4 text, col5 text,
            col6 text, col7 text, col8 text, col9 text, col10 text
        )", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert rows with large data - each row ~5KB, 10 rows = ~50KB total
    for (int i = 0; i < 10; i++)
    {
        using (var cmd = new NpgsqlCommand(@"
            INSERT INTO test_stress_describe(col1, col2, col3, col4, col5, col6, col7, col8, col9, col10) 
            VALUES(@c1, @c2, @c3, @c4, @c5, @c6, @c7, @c8, @c9, @c10)", connection))
        {
            for (int c = 1; c <= 10; c++)
            {
                cmd.Parameters.AddWithValue($"c{c}", new string((char)('A' + c), 500));
            }
            cmd.ExecuteNonQuery();
        }
    }

    // Rapid prepare/execute cycle - each Prepare triggers Parse+Describe
    for (int cycle = 0; cycle < 50; cycle++)
    {
        using (var cmd = new NpgsqlCommand(
            "SELECT * FROM test_stress_describe WHERE id > @id ORDER BY id", connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Prepare(); // This sends Parse + Describe, expects ParameterDescription + RowDescription
            
            cmd.Parameters["id"].Value = 0;
            int count = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 10) throw new Exception($"Cycle {cycle}: Expected 10, got {count}");
        }
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Parallel clients all doing Prepare simultaneously
// This maximizes the chance of race conditions in ParseComplete insertion
Console.WriteLine("Test 2: Parallel Prepare stress test");
{
    var errors = new List<string>();
    var lockObj = new object();
    var barrier = new Barrier(16); // Synchronize all clients to prepare at the same time
    var tasks = new List<Task>();

    for (int clientId = 0; clientId < 16; clientId++)
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
                    // Wait for all clients to be ready
                    barrier.SignalAndWait();

                    // All clients prepare at the same time
                    await using var cmd = new NpgsqlCommand(
                        "SELECT * FROM test_stress_describe WHERE id > @id ORDER BY id", conn);
                    cmd.Parameters.Add("id", NpgsqlDbType.Integer);
                    await cmd.PrepareAsync(); // Simultaneous Parse+Describe from all clients

                    cmd.Parameters["id"].Value = id % 5;
                    int count = 0;
                    await using (var reader = await cmd.ExecuteReaderAsync())
                    {
                        while (await reader.ReadAsync()) count++;
                    }
                    
                    int expected = 10 - (id % 5);
                    if (count != expected)
                    {
                        lock (lockObj)
                        {
                            errors.Add($"Client {id} round {round}: Expected {expected}, got {count}");
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
        throw new Exception($"Parallel prepare test failed with {errors.Count} errors");
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Interleaved Prepare and Execute with different query patterns
// Tests that ParseComplete is correctly matched to the right statement
Console.WriteLine("Test 3: Interleaved Prepare/Execute patterns");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    for (int iteration = 0; iteration < 20; iteration++)
    {
        // Prepare statement A
        var cmdA = new NpgsqlCommand("SELECT col1, col2, col3 FROM test_stress_describe WHERE id = @id", connection);
        cmdA.Parameters.Add("id", NpgsqlDbType.Integer);
        cmdA.Prepare();

        // Prepare statement B (different columns)
        var cmdB = new NpgsqlCommand("SELECT col4, col5, col6, col7 FROM test_stress_describe WHERE id > @id", connection);
        cmdB.Parameters.Add("id", NpgsqlDbType.Integer);
        cmdB.Prepare();

        // Prepare statement C (all columns - large result)
        var cmdC = new NpgsqlCommand("SELECT * FROM test_stress_describe ORDER BY id", connection);
        cmdC.Prepare();

        // Execute in different order than prepared
        // Execute C first (large result)
        int countC = 0;
        using (var reader = cmdC.ExecuteReader())
        {
            while (reader.Read()) countC++;
        }
        if (countC != 10) throw new Exception($"Iter {iteration} C: Expected 10, got {countC}");

        // Execute A
        cmdA.Parameters["id"].Value = 1;
        using (var reader = cmdA.ExecuteReader())
        {
            if (!reader.Read()) throw new Exception($"Iter {iteration} A: No row");
            // Verify we got 3 columns
            if (reader.FieldCount != 3) throw new Exception($"Iter {iteration} A: Expected 3 columns, got {reader.FieldCount}");
        }

        // Execute B
        cmdB.Parameters["id"].Value = 5;
        int countB = 0;
        using (var reader = cmdB.ExecuteReader())
        {
            while (reader.Read())
            {
                // Verify we got 4 columns
                if (reader.FieldCount != 4) throw new Exception($"Iter {iteration} B: Expected 4 columns, got {reader.FieldCount}");
                countB++;
            }
        }
        if (countB != 5) throw new Exception($"Iter {iteration} B: Expected 5, got {countB}");

        cmdA.Dispose();
        cmdB.Dispose();
        cmdC.Dispose();
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Prepare with CommandBehavior.SchemaOnly (explicit Describe without Execute)
// This specifically tests the Describe path
Console.WriteLine("Test 4: SchemaOnly Describe test");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    for (int i = 0; i < 30; i++)
    {
        using (var cmd = new NpgsqlCommand("SELECT * FROM test_stress_describe WHERE id > @id", connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Parameters["id"].Value = 0;
            
            // SchemaOnly triggers Describe to get column info without fetching data
            using (var reader = cmd.ExecuteReader(CommandBehavior.SchemaOnly))
            {
                var schema = reader.GetSchemaTable();
                if (schema == null) throw new Exception($"Iter {i}: No schema");
                if (schema.Rows.Count != 11) throw new Exception($"Iter {i}: Expected 11 columns, got {schema.Rows.Count}");
            }
        }

        // Immediately follow with a full query (large result)
        using (var cmd = new NpgsqlCommand("SELECT * FROM test_stress_describe ORDER BY id", connection))
        {
            cmd.Prepare();
            int count = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 10) throw new Exception($"Iter {i}: Expected 10, got {count}");
        }
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Mixed named and unnamed prepared statements with large data
Console.WriteLine("Test 5: Mixed named/unnamed prepared statements");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    for (int round = 0; round < 15; round++)
    {
        // Unnamed prepared (auto-prepared by Npgsql)
        using (var unnamed = new NpgsqlCommand("SELECT * FROM test_stress_describe WHERE id = @id", connection))
        {
            unnamed.Parameters.AddWithValue("id", 1);
            using (var reader = unnamed.ExecuteReader())
            {
                if (!reader.Read()) throw new Exception($"Round {round}: Unnamed no row");
            }
        }

        // Explicitly prepared (named)
        using (var named = new NpgsqlCommand("SELECT * FROM test_stress_describe ORDER BY id", connection))
        {
            named.Prepare();
            int count = 0;
            using (var reader = named.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 10) throw new Exception($"Round {round}: Named expected 10, got {count}");
        }

        // Another unnamed with different query
        using (var unnamed2 = new NpgsqlCommand("SELECT col1, col2 FROM test_stress_describe WHERE id > @id", connection))
        {
            unnamed2.Parameters.AddWithValue("id", 5);
            int count = 0;
            using (var reader = unnamed2.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 5) throw new Exception($"Round {round}: Unnamed2 expected 5, got {count}");
        }
    }
}
Console.WriteLine("Test 5 complete");

// Test 6: Concurrent connections with different prepared statement lifecycles
Console.WriteLine("Test 6: Concurrent connections different lifecycles");
{
    var errors = new List<string>();
    var lockObj = new object();
    var tasks = new List<Task>();

    for (int clientId = 0; clientId < 10; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();

                // Client-specific behavior based on id
                if (id % 3 == 0)
                {
                    // Long-lived prepared statement
                    await using var cmd = new NpgsqlCommand(
                        "SELECT * FROM test_stress_describe ORDER BY id", conn);
                    await cmd.PrepareAsync();

                    for (int i = 0; i < 20; i++)
                    {
                        int count = 0;
                        await using (var reader = await cmd.ExecuteReaderAsync())
                        {
                            while (await reader.ReadAsync()) count++;
                        }
                        if (count != 10)
                        {
                            lock (lockObj) { errors.Add($"Client {id} iter {i}: Expected 10, got {count}"); }
                        }
                    }
                }
                else if (id % 3 == 1)
                {
                    // Short-lived prepared statements
                    for (int i = 0; i < 20; i++)
                    {
                        await using var cmd = new NpgsqlCommand(
                            "SELECT * FROM test_stress_describe WHERE id > @id", conn);
                        cmd.Parameters.Add("id", NpgsqlDbType.Integer);
                        await cmd.PrepareAsync();

                        cmd.Parameters["id"].Value = i % 5;
                        int count = 0;
                        await using (var reader = await cmd.ExecuteReaderAsync())
                        {
                            while (await reader.ReadAsync()) count++;
                        }
                        int expected = 10 - (i % 5);
                        if (count != expected)
                        {
                            lock (lockObj) { errors.Add($"Client {id} iter {i}: Expected {expected}, got {count}"); }
                        }
                    }
                }
                else
                {
                    // Unprepared queries only
                    for (int i = 0; i < 20; i++)
                    {
                        await using var cmd = new NpgsqlCommand(
                            $"SELECT * FROM test_stress_describe WHERE id > {i % 5} ORDER BY id", conn);
                        int count = 0;
                        await using (var reader = await cmd.ExecuteReaderAsync())
                        {
                            while (await reader.ReadAsync()) count++;
                        }
                        int expected = 10 - (i % 5);
                        if (count != expected)
                        {
                            lock (lockObj) { errors.Add($"Client {id} iter {i}: Expected {expected}, got {count}"); }
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
        throw new Exception($"Concurrent lifecycle test failed with {errors.Count} errors");
    }
}
Console.WriteLine("Test 6 complete");

// Test 7: Prepare with very large parameter count (stress ParameterDescription)
Console.WriteLine("Test 7: Large parameter count stress test");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create table with many columns
    var createSql = new StringBuilder("DROP TABLE IF EXISTS test_many_params; CREATE TABLE test_many_params(id serial primary key");
    for (int i = 1; i <= 20; i++)
    {
        createSql.Append($", p{i} text");
    }
    createSql.Append(")");

    using (var cmd = new NpgsqlCommand(createSql.ToString(), connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert with many parameters
    var insertSql = new StringBuilder("INSERT INTO test_many_params(");
    var valuesSql = new StringBuilder(" VALUES(");
    for (int i = 1; i <= 20; i++)
    {
        if (i > 1) { insertSql.Append(", "); valuesSql.Append(", "); }
        insertSql.Append($"p{i}");
        valuesSql.Append($"@p{i}");
    }
    insertSql.Append(")");
    valuesSql.Append(")");

    for (int row = 0; row < 10; row++)
    {
        using (var cmd = new NpgsqlCommand(insertSql.ToString() + valuesSql.ToString(), connection))
        {
            for (int i = 1; i <= 20; i++)
            {
                cmd.Parameters.AddWithValue($"p{i}", new string((char)('A' + i), 200));
            }
            cmd.Prepare(); // Large ParameterDescription response
            cmd.ExecuteNonQuery();
        }
    }

    // Select all (large result with many columns)
    using (var cmd = new NpgsqlCommand("SELECT * FROM test_many_params ORDER BY id", connection))
    {
        cmd.Prepare();
        int count = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read())
            {
                if (reader.FieldCount != 21) throw new Exception($"Expected 21 columns, got {reader.FieldCount}");
                count++;
            }
        }
        if (count != 10) throw new Exception($"Expected 10 rows, got {count}");
    }
}
Console.WriteLine("Test 7 complete");

// Test 8: Rapid connection open/close with prepared statements
Console.WriteLine("Test 8: Rapid connection cycling with prepared statements");
for (int cycle = 0; cycle < 30; cycle++)
{
    using (var connection = new NpgsqlConnection(connectionString))
    {
        connection.Open();

        using (var cmd = new NpgsqlCommand("SELECT * FROM test_stress_describe ORDER BY id", connection))
        {
            cmd.Prepare();
            int count = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 10) throw new Exception($"Cycle {cycle}: Expected 10, got {count}");
        }
    }
    // Connection closed, prepared statement deallocated
}
Console.WriteLine("Test 8 complete");

// Test 9: Pipelining simulation - multiple commands without waiting
Console.WriteLine("Test 9: Pipelining simulation");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Npgsql supports batching which uses pipelining
    for (int batch = 0; batch < 10; batch++)
    {
        using (var batchCmd = new NpgsqlBatch(connection))
        {
            // Add multiple commands to batch
            for (int i = 0; i < 5; i++)
            {
                var cmd = new NpgsqlBatchCommand($"SELECT * FROM test_stress_describe WHERE id > {i} ORDER BY id");
                batchCmd.BatchCommands.Add(cmd);
            }

            // Execute all at once
            using (var reader = batchCmd.ExecuteReader())
            {
                int cmdIndex = 0;
                do
                {
                    int count = 0;
                    while (reader.Read()) count++;
                    int expected = 10 - cmdIndex;
                    if (count != expected)
                    {
                        throw new Exception($"Batch {batch} cmd {cmdIndex}: Expected {expected}, got {count}");
                    }
                    cmdIndex++;
                } while (reader.NextResult());

                if (cmdIndex != 5) throw new Exception($"Batch {batch}: Expected 5 results, got {cmdIndex}");
            }
        }
    }
}
Console.WriteLine("Test 9 complete");

// Test 10: Extended protocol with explicit Describe portal
Console.WriteLine("Test 10: Extended protocol stress");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    for (int i = 0; i < 30; i++)
    {
        // Create prepared statement
        using (var cmd = new NpgsqlCommand("SELECT * FROM test_stress_describe WHERE id > @id ORDER BY id", connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Parameters["id"].Value = i % 5;

            // Prepare sends Parse + Describe(Statement)
            cmd.Prepare();

            // First execution - Bind + Describe(Portal) + Execute
            int count1 = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count1++;
            }

            // Second execution with different parameter - Bind + Execute
            cmd.Parameters["id"].Value = (i + 2) % 5;
            int count2 = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count2++;
            }

            int expected1 = 10 - (i % 5);
            int expected2 = 10 - ((i + 2) % 5);
            if (count1 != expected1) throw new Exception($"Iter {i} exec1: Expected {expected1}, got {count1}");
            if (count2 != expected2) throw new Exception($"Iter {i} exec2: Expected {expected2}, got {count2}");
        }
    }
}
Console.WriteLine("Test 10 complete");

Console.WriteLine("prepared_stress_describe complete");
