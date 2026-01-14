using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Data;
using System.Text;
using System.Threading.Tasks;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Named prepared statement with explicit Prepare and large result
Console.WriteLine("Test 1: Named prepared statement with Describe and large result");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create table with large data
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_named_prep;
        CREATE TABLE test_named_prep(id serial primary key, name text, data text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert large data
    using (var cmd = new NpgsqlCommand("INSERT INTO test_named_prep(name, data) VALUES(@name, @data)", connection))
    {
        cmd.Parameters.Add("name", NpgsqlDbType.Text);
        cmd.Parameters.Add("data", NpgsqlDbType.Text);
        cmd.Prepare(); // This triggers Parse + Describe

        for (int i = 0; i < 30; i++)
        {
            cmd.Parameters["name"].Value = $"item_{i}";
            cmd.Parameters["data"].Value = new string('X', 500) + $"_{i}"; // ~500 bytes per row
            cmd.ExecuteNonQuery();
        }
    }

    // Select with prepared statement - result >8KB
    using (var cmd = new NpgsqlCommand("SELECT id, name, data FROM test_named_prep WHERE id > @minId ORDER BY id", connection))
    {
        cmd.Parameters.Add("minId", NpgsqlDbType.Integer);
        cmd.Prepare(); // Parse + Describe for SELECT

        cmd.Parameters["minId"].Value = 0;
        int rowCount = 0;
        int totalBytes = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read())
            {
                totalBytes += reader.GetString(1).Length + reader.GetString(2).Length;
                rowCount++;
            }
        }
        if (rowCount != 30) throw new Exception($"Expected 30 rows, got {rowCount}");
        Console.WriteLine($"  Retrieved {rowCount} rows, {totalBytes} bytes");
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Multiple prepared statements with interleaved execution and large data
Console.WriteLine("Test 2: Interleaved prepared statements with large data");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Prepare multiple statements
    var selectAll = new NpgsqlCommand("SELECT * FROM test_named_prep ORDER BY id", connection);
    selectAll.Prepare();

    var selectById = new NpgsqlCommand("SELECT * FROM test_named_prep WHERE id = @id", connection);
    selectById.Parameters.Add("id", NpgsqlDbType.Integer);
    selectById.Prepare();

    var selectRange = new NpgsqlCommand("SELECT * FROM test_named_prep WHERE id BETWEEN @min AND @max ORDER BY id", connection);
    selectRange.Parameters.Add("min", NpgsqlDbType.Integer);
    selectRange.Parameters.Add("max", NpgsqlDbType.Integer);
    selectRange.Prepare();

    // Interleave executions with large results
    for (int iteration = 0; iteration < 5; iteration++)
    {
        // Execute selectAll (large result >8KB)
        int count1 = 0;
        using (var reader = selectAll.ExecuteReader())
        {
            while (reader.Read()) count1++;
        }
        if (count1 != 30) throw new Exception($"selectAll: Expected 30, got {count1}");

        // Execute selectById (small result)
        selectById.Parameters["id"].Value = iteration + 1;
        using (var reader = selectById.ExecuteReader())
        {
            if (!reader.Read()) throw new Exception($"selectById: No row for id={iteration + 1}");
        }

        // Execute selectRange (medium result)
        selectRange.Parameters["min"].Value = 1;
        selectRange.Parameters["max"].Value = 15;
        int count3 = 0;
        using (var reader = selectRange.ExecuteReader())
        {
            while (reader.Read()) count3++;
        }
        if (count3 != 15) throw new Exception($"selectRange: Expected 15, got {count3}");
    }

    selectAll.Dispose();
    selectById.Dispose();
    selectRange.Dispose();
}
Console.WriteLine("Test 2 complete");

// Test 3: Prepared statement reuse across unprepared queries with large data
Console.WriteLine("Test 3: Mixed prepared/unprepared with large results");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    var prepared = new NpgsqlCommand("SELECT * FROM test_named_prep WHERE id > @id ORDER BY id", connection);
    prepared.Parameters.Add("id", NpgsqlDbType.Integer);
    prepared.Prepare();

    for (int i = 0; i < 10; i++)
    {
        // Execute prepared (large result)
        prepared.Parameters["id"].Value = 0;
        int preparedCount = 0;
        using (var reader = prepared.ExecuteReader())
        {
            while (reader.Read()) preparedCount++;
        }
        if (preparedCount != 30) throw new Exception($"Prepared: Expected 30, got {preparedCount}");

        // Execute unprepared query in between (also large result)
        using (var unprepared = new NpgsqlCommand($"SELECT * FROM test_named_prep WHERE name LIKE 'item_%' ORDER BY id", connection))
        {
            int unpreparedCount = 0;
            using (var reader = unprepared.ExecuteReader())
            {
                while (reader.Read()) unpreparedCount++;
            }
            if (unpreparedCount != 30) throw new Exception($"Unprepared: Expected 30, got {unpreparedCount}");
        }
    }

    prepared.Dispose();
}
Console.WriteLine("Test 3 complete");

// Test 4: Parallel connections with same prepared statement pattern
Console.WriteLine("Test 4: Parallel connections with prepared statements");
{
    var tasks = new List<Task>();
    var errors = new List<string>();
    var lockObj = new object();

    for (int clientId = 0; clientId < 8; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();

                // Each client prepares the same query pattern
                await using var cmd = new NpgsqlCommand("SELECT * FROM test_named_prep WHERE id > @id ORDER BY id", conn);
                cmd.Parameters.Add("id", NpgsqlDbType.Integer);
                await cmd.PrepareAsync();

                for (int iter = 0; iter < 10; iter++)
                {
                    cmd.Parameters["id"].Value = id % 10; // Different starting points
                    int count = 0;
                    await using (var reader = await cmd.ExecuteReaderAsync())
                    {
                        while (await reader.ReadAsync()) count++;
                    }
                    // Verify we got expected rows
                    int expected = 30 - (id % 10);
                    if (count != expected)
                    {
                        lock (lockObj)
                        {
                            errors.Add($"Client {id} iter {iter}: Expected {expected}, got {count}");
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                lock (lockObj)
                {
                    errors.Add($"Client {id}: {ex.Message}");
                }
            }
        }));
    }

    Task.WaitAll(tasks.ToArray());

    if (errors.Count > 0)
    {
        foreach (var err in errors) Console.WriteLine($"  ERROR: {err}");
        throw new Exception($"Parallel test failed with {errors.Count} errors");
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Prepared statement with schema introspection (triggers Describe)
Console.WriteLine("Test 5: Schema introspection with prepared statements");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Use GetSchema which may trigger internal prepared statements
    var tables = connection.GetSchema("Tables");
    Console.WriteLine($"  Found {tables.Rows.Count} tables");

    // Now execute prepared statement with large result
    using (var cmd = new NpgsqlCommand("SELECT * FROM test_named_prep ORDER BY id", connection))
    {
        cmd.Prepare();
        int count = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read()) count++;
        }
        if (count != 30) throw new Exception($"Expected 30, got {count}");
    }

    // Get columns schema
    var columns = connection.GetSchema("Columns", new[] { null, null, "test_named_prep" });
    Console.WriteLine($"  Found {columns.Rows.Count} columns in test_named_prep");
}
Console.WriteLine("Test 5 complete");

// Test 6: Rapid prepare/execute/close cycle with large data
Console.WriteLine("Test 6: Rapid prepare/execute/close cycle");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    for (int cycle = 0; cycle < 20; cycle++)
    {
        using (var cmd = new NpgsqlCommand("SELECT * FROM test_named_prep WHERE id > @id ORDER BY id", connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Prepare(); // Parse + Describe

            cmd.Parameters["id"].Value = cycle % 10;
            int count = 0;
            using (var reader = cmd.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            int expected = 30 - (cycle % 10);
            if (count != expected) throw new Exception($"Cycle {cycle}: Expected {expected}, got {count}");
        }
        // Command disposed - Close sent
    }
}
Console.WriteLine("Test 6 complete");

// Test 7: Prepared INSERT followed by prepared SELECT with large data
Console.WriteLine("Test 7: Prepared INSERT then SELECT with large data");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create fresh table
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_insert_select;
        CREATE TABLE test_insert_select(id serial primary key, payload text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Prepared INSERT
    using (var insertCmd = new NpgsqlCommand("INSERT INTO test_insert_select(payload) VALUES(@payload)", connection))
    {
        insertCmd.Parameters.Add("payload", NpgsqlDbType.Text);
        insertCmd.Prepare();

        for (int i = 0; i < 50; i++)
        {
            insertCmd.Parameters["payload"].Value = new string((char)('A' + (i % 26)), 300) + $"_{i}";
            insertCmd.ExecuteNonQuery();
        }
    }

    // Prepared SELECT immediately after (large result >8KB)
    using (var selectCmd = new NpgsqlCommand("SELECT * FROM test_insert_select ORDER BY id", connection))
    {
        selectCmd.Prepare();
        int count = 0;
        int totalBytes = 0;
        using (var reader = selectCmd.ExecuteReader())
        {
            while (reader.Read())
            {
                totalBytes += reader.GetString(1).Length;
                count++;
            }
        }
        if (count != 50) throw new Exception($"Expected 50, got {count}");
        Console.WriteLine($"  Retrieved {count} rows, {totalBytes} bytes");
    }
}
Console.WriteLine("Test 7 complete");

// Test 8: Multiple prepared statements created before any execution
Console.WriteLine("Test 8: Batch prepare then batch execute");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create all prepared statements first
    var commands = new List<NpgsqlCommand>();
    for (int i = 0; i < 5; i++)
    {
        var cmd = new NpgsqlCommand($"SELECT * FROM test_named_prep WHERE id > @id ORDER BY id LIMIT {(i + 1) * 10}", connection);
        cmd.Parameters.Add("id", NpgsqlDbType.Integer);
        cmd.Prepare();
        commands.Add(cmd);
    }

    // Now execute all in various orders
    for (int round = 0; round < 3; round++)
    {
        for (int i = commands.Count - 1; i >= 0; i--)
        {
            commands[i].Parameters["id"].Value = 0;
            int count = 0;
            using (var reader = commands[i].ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            int expected = Math.Min((i + 1) * 10, 30);
            if (count != expected) throw new Exception($"Round {round} cmd {i}: Expected {expected}, got {count}");
        }
    }

    foreach (var cmd in commands) cmd.Dispose();
}
Console.WriteLine("Test 8 complete");

// Test 9: Prepared statement with NULL parameters and large result
Console.WriteLine("Test 9: NULL parameters with large result");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        SELECT * FROM test_named_prep 
        WHERE (@filter IS NULL OR name LIKE @filter)
        ORDER BY id", connection))
    {
        cmd.Parameters.Add("filter", NpgsqlDbType.Text);
        cmd.Prepare();

        // First with NULL (returns all rows - large result)
        cmd.Parameters["filter"].Value = DBNull.Value;
        int count1 = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read()) count1++;
        }
        if (count1 != 30) throw new Exception($"NULL filter: Expected 30, got {count1}");

        // Then with actual filter
        cmd.Parameters["filter"].Value = "item_1%";
        int count2 = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read()) count2++;
        }
        // item_1, item_10-19 = 11 rows
        if (count2 != 11) throw new Exception($"Filter 'item_1%': Expected 11, got {count2}");
    }
}
Console.WriteLine("Test 9 complete");

// Test 10: Transaction with prepared statements and large data
Console.WriteLine("Test 10: Transaction with prepared statements");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var transaction = connection.BeginTransaction())
    {
        // Prepared INSERT in transaction
        using (var insertCmd = new NpgsqlCommand("INSERT INTO test_named_prep(name, data) VALUES(@name, @data)", connection, transaction))
        {
            insertCmd.Parameters.Add("name", NpgsqlDbType.Text);
            insertCmd.Parameters.Add("data", NpgsqlDbType.Text);
            insertCmd.Prepare();

            for (int i = 0; i < 10; i++)
            {
                insertCmd.Parameters["name"].Value = $"tx_item_{i}";
                insertCmd.Parameters["data"].Value = new string('T', 500);
                insertCmd.ExecuteNonQuery();
            }
        }

        // Prepared SELECT in same transaction (large result)
        using (var selectCmd = new NpgsqlCommand("SELECT * FROM test_named_prep ORDER BY id", connection, transaction))
        {
            selectCmd.Prepare();
            int count = 0;
            using (var reader = selectCmd.ExecuteReader())
            {
                while (reader.Read()) count++;
            }
            if (count != 40) throw new Exception($"In transaction: Expected 40, got {count}");
        }

        transaction.Rollback(); // Don't actually commit
    }

    // Verify rollback worked
    using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_named_prep", connection))
    {
        var count = (long)cmd.ExecuteScalar()!;
        if (count != 30) throw new Exception($"After rollback: Expected 30, got {count}");
    }
}
Console.WriteLine("Test 10 complete");

Console.WriteLine("prepared_named_describe complete");
