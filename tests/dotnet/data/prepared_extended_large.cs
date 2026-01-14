using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Text;
using System.Threading.Tasks;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Large result set (>8196 bytes) with prepared statement
Console.WriteLine("Test 1: Large result set with prepared statement");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create table with large text data
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_large_data;
        CREATE TABLE test_large_data(id serial primary key, large_text text, data bytea)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert rows with large text (each row ~1KB, insert 20 rows = ~20KB total)
    using (var cmd = new NpgsqlCommand("INSERT INTO test_large_data(large_text, data) VALUES(@text, @data)", connection))
    {
        cmd.Parameters.Add("text", NpgsqlDbType.Text);
        cmd.Parameters.Add("data", NpgsqlDbType.Bytea);
        cmd.Prepare();

        for (int i = 0; i < 20; i++)
        {
            // Create ~1KB of text data
            var largeText = new string((char)('A' + (i % 26)), 1000) + $"_row_{i}";
            var binaryData = new byte[500];
            new Random(i).NextBytes(binaryData);
            
            cmd.Parameters["text"].Value = largeText;
            cmd.Parameters["data"].Value = binaryData;
            cmd.ExecuteNonQuery();
        }
    }

    // Select all data (should be >8196 bytes)
    using (var cmd = new NpgsqlCommand("SELECT * FROM test_large_data ORDER BY id", connection))
    {
        cmd.Prepare();
        
        int totalBytes = 0;
        int rowCount = 0;
        using (var reader = cmd.ExecuteReader())
        {
            while (reader.Read())
            {
                var text = reader.GetString(1);
                var data = (byte[])reader.GetValue(2);
                totalBytes += text.Length + data.Length;
                rowCount++;
            }
        }
        
        if (rowCount != 20) throw new Exception($"Expected 20 rows, got {rowCount}");
        if (totalBytes < 8196) throw new Exception($"Expected >8196 bytes, got {totalBytes}");
        Console.WriteLine($"  Retrieved {rowCount} rows, {totalBytes} bytes total");
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Multiple parallel clients with prepared statements and large data
Console.WriteLine("Test 2: Parallel clients with large data");
{
    var tasks = new List<Task>();
    var errors = new List<string>();
    var lockObj = new object();
    
    for (int clientId = 0; clientId < 5; clientId++)
    {
        int id = clientId;
        tasks.Add(Task.Run(async () =>
        {
            try
            {
                await using var conn = new NpgsqlConnection(connectionString);
                await conn.OpenAsync();
                
                // Each client creates its own table
                var tableName = $"test_parallel_large_{id}";
                await using (var cmd = new NpgsqlCommand($@"
                    DROP TABLE IF EXISTS {tableName};
                    CREATE TABLE {tableName}(id serial primary key, data text)", conn))
                {
                    await cmd.ExecuteNonQueryAsync();
                }
                
                // Insert large data with prepared statement
                await using (var insertCmd = new NpgsqlCommand($"INSERT INTO {tableName}(data) VALUES(@data)", conn))
                {
                    insertCmd.Parameters.Add("data", NpgsqlDbType.Text);
                    await insertCmd.PrepareAsync();
                    
                    for (int i = 0; i < 10; i++)
                    {
                        // ~2KB per row
                        insertCmd.Parameters["data"].Value = new string((char)('A' + id), 2000) + $"_{i}";
                        await insertCmd.ExecuteNonQueryAsync();
                    }
                }
                
                // Select with prepared statement (>8196 bytes result)
                await using (var selectCmd = new NpgsqlCommand($"SELECT * FROM {tableName} ORDER BY id", conn))
                {
                    await selectCmd.PrepareAsync();
                    
                    int rows = 0;
                    int bytes = 0;
                    await using (var reader = await selectCmd.ExecuteReaderAsync())
                    {
                        while (await reader.ReadAsync())
                        {
                            bytes += reader.GetString(1).Length;
                            rows++;
                        }
                    }
                    
                    if (rows != 10) throw new Exception($"Client {id}: Expected 10 rows, got {rows}");
                    if (bytes < 8196) throw new Exception($"Client {id}: Expected >8196 bytes, got {bytes}");
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
        throw new Exception($"Parallel test failed:\n{string.Join("\n", errors)}");
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Mixed prepared and unprepared queries with large results
Console.WriteLine("Test 3: Mixed prepared/unprepared with large results");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Setup
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_mixed_large;
        CREATE TABLE test_mixed_large(id serial primary key, payload text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Prepared insert
    using (var preparedInsert = new NpgsqlCommand("INSERT INTO test_mixed_large(payload) VALUES(@p)", connection))
    {
        preparedInsert.Parameters.Add("p", NpgsqlDbType.Text);
        preparedInsert.Prepare();
        
        for (int i = 0; i < 15; i++)
        {
            preparedInsert.Parameters["p"].Value = new string('X', 1500) + $"_prepared_{i}";
            preparedInsert.ExecuteNonQuery();
        }
    }

    // Unprepared insert (extended protocol still used by Npgsql)
    for (int i = 0; i < 15; i++)
    {
        using (var unpreparedInsert = new NpgsqlCommand("INSERT INTO test_mixed_large(payload) VALUES(@p)", connection))
        {
            unpreparedInsert.Parameters.AddWithValue("p", new string('Y', 1500) + $"_unprepared_{i}");
            unpreparedInsert.ExecuteNonQuery();
        }
    }

    // Prepared select - exclude unprepared rows
    using (var preparedSelect = new NpgsqlCommand("SELECT * FROM test_mixed_large WHERE payload NOT LIKE '%unprepared%' ORDER BY id", connection))
    {
        preparedSelect.Prepare();
        
        int count = 0;
        int bytes = 0;
        using (var reader = preparedSelect.ExecuteReader())
        {
            while (reader.Read())
            {
                bytes += reader.GetString(1).Length;
                count++;
            }
        }
        if (count != 15) throw new Exception($"Expected 15 prepared rows, got {count}");
        Console.WriteLine($"  Prepared select: {count} rows, {bytes} bytes");
    }

    // Unprepared select
    using (var unpreparedSelect = new NpgsqlCommand("SELECT * FROM test_mixed_large ORDER BY id", connection))
    {
        int count = 0;
        int bytes = 0;
        using (var reader = unpreparedSelect.ExecuteReader())
        {
            while (reader.Read())
            {
                bytes += reader.GetString(1).Length;
                count++;
            }
        }
        if (count != 30) throw new Exception($"Expected 30 total rows, got {count}");
        if (bytes < 8196) throw new Exception($"Expected >8196 bytes, got {bytes}");
        Console.WriteLine($"  Unprepared select: {count} rows, {bytes} bytes");
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Concurrent prepared statements on same connection with large data
Console.WriteLine("Test 4: Concurrent prepared statements same connection");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Setup multiple tables
    for (int t = 0; t < 3; t++)
    {
        using (var cmd = new NpgsqlCommand($@"
            DROP TABLE IF EXISTS test_concurrent_prep_{t};
            CREATE TABLE test_concurrent_prep_{t}(id serial primary key, val text)", connection))
        {
            cmd.ExecuteNonQuery();
        }
    }

    // Create multiple prepared statements
    var preparedCmds = new List<NpgsqlCommand>();
    for (int t = 0; t < 3; t++)
    {
        var cmd = new NpgsqlCommand($"INSERT INTO test_concurrent_prep_{t}(val) VALUES(@v)", connection);
        cmd.Parameters.Add("v", NpgsqlDbType.Text);
        cmd.Prepare();
        preparedCmds.Add(cmd);
    }

    // Interleave executions with large data
    for (int i = 0; i < 10; i++)
    {
        for (int t = 0; t < 3; t++)
        {
            preparedCmds[t].Parameters["v"].Value = new string((char)('A' + t), 1000) + $"_iter_{i}";
            preparedCmds[t].ExecuteNonQuery();
        }
    }

    // Cleanup prepared commands
    foreach (var cmd in preparedCmds)
    {
        cmd.Dispose();
    }

    // Verify with prepared selects returning large data
    for (int t = 0; t < 3; t++)
    {
        using (var selectCmd = new NpgsqlCommand($"SELECT * FROM test_concurrent_prep_{t} ORDER BY id", connection))
        {
            selectCmd.Prepare();
            
            int count = 0;
            int bytes = 0;
            using (var reader = selectCmd.ExecuteReader())
            {
                while (reader.Read())
                {
                    bytes += reader.GetString(1).Length;
                    count++;
                }
            }
            if (count != 10) throw new Exception($"Table {t}: Expected 10 rows, got {count}");
            if (bytes < 8196) throw new Exception($"Table {t}: Expected >8196 bytes, got {bytes}");
        }
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Prepared statement with very large single row (>8196 bytes)
Console.WriteLine("Test 5: Very large single row");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_very_large;
        CREATE TABLE test_very_large(id serial primary key, huge_text text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert single row with >10KB of data
    using (var insertCmd = new NpgsqlCommand("INSERT INTO test_very_large(huge_text) VALUES(@t)", connection))
    {
        insertCmd.Parameters.Add("t", NpgsqlDbType.Text);
        insertCmd.Prepare();
        
        // Create 15KB of text
        var hugeText = new StringBuilder();
        for (int i = 0; i < 15; i++)
        {
            hugeText.Append(new string((char)('A' + i % 26), 1000));
            hugeText.Append($"_block_{i}_");
        }
        
        insertCmd.Parameters["t"].Value = hugeText.ToString();
        insertCmd.ExecuteNonQuery();
    }

    // Select with prepared statement
    using (var selectCmd = new NpgsqlCommand("SELECT huge_text FROM test_very_large", connection))
    {
        selectCmd.Prepare();
        
        var result = (string)selectCmd.ExecuteScalar()!;
        if (result.Length < 15000) throw new Exception($"Expected >15000 chars, got {result.Length}");
        Console.WriteLine($"  Retrieved single row with {result.Length} characters");
    }
}
Console.WriteLine("Test 5 complete");

// Test 6: Multiple connections with same prepared statement name pattern
Console.WriteLine("Test 6: Multiple connections same prepared pattern");
{
    var tasks = new List<Task>();
    var results = new int[10];
    
    for (int i = 0; i < 10; i++)
    {
        int idx = i;
        tasks.Add(Task.Run(async () =>
        {
            await using var conn = new NpgsqlConnection(connectionString);
            await conn.OpenAsync();
            
            // Each connection uses same query pattern
            await using var cmd = new NpgsqlCommand("SELECT LENGTH(REPEAT(@char, @count))", conn);
            cmd.Parameters.Add("char", NpgsqlDbType.Text).Value = new string('Z', 100);
            cmd.Parameters.Add("count", NpgsqlDbType.Integer).Value = 100 + idx * 10;
            await cmd.PrepareAsync();
            
            results[idx] = (int)(await cmd.ExecuteScalarAsync())!;
        }));
    }
    
    Task.WaitAll(tasks.ToArray());
    
    for (int i = 0; i < 10; i++)
    {
        int expected = 100 * (100 + i * 10);
        if (results[i] != expected) throw new Exception($"Connection {i}: Expected {expected}, got {results[i]}");
    }
}
Console.WriteLine("Test 6 complete");

// Test 7: Streaming large result with multiple DataRow messages
Console.WriteLine("Test 7: Streaming large result");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_stream_large;
        CREATE TABLE test_stream_large(id serial primary key, chunk text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // Insert 100 rows with ~500 bytes each = ~50KB total
    using (var insertCmd = new NpgsqlCommand("INSERT INTO test_stream_large(chunk) VALUES(@c)", connection))
    {
        insertCmd.Parameters.Add("c", NpgsqlDbType.Text);
        insertCmd.Prepare();
        
        for (int i = 0; i < 100; i++)
        {
            insertCmd.Parameters["c"].Value = new string((char)('A' + i % 26), 500) + $"_{i:D4}";
            insertCmd.ExecuteNonQuery();
        }
    }

    // Stream all rows with prepared statement
    using (var selectCmd = new NpgsqlCommand("SELECT * FROM test_stream_large ORDER BY id", connection))
    {
        selectCmd.Prepare();
        
        int rowCount = 0;
        long totalBytes = 0;
        using (var reader = selectCmd.ExecuteReader())
        {
            while (reader.Read())
            {
                totalBytes += reader.GetString(1).Length;
                rowCount++;
            }
        }
        
        if (rowCount != 100) throw new Exception($"Expected 100 rows, got {rowCount}");
        if (totalBytes < 50000) throw new Exception($"Expected >50000 bytes, got {totalBytes}");
        Console.WriteLine($"  Streamed {rowCount} rows, {totalBytes} bytes");
    }
}
Console.WriteLine("Test 7 complete");

Console.WriteLine("prepared_extended_large complete");
