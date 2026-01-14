using Npgsql;
using NpgsqlTypes;
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Multiple sequential connections
Console.WriteLine("Test 1: Multiple sequential connections");
for (int i = 0; i < 10; i++)
{
    using (var connection = new NpgsqlConnection(connectionString))
    {
        connection.Open();
        using (var cmd = new NpgsqlCommand($"SELECT {i} as val", connection))
        {
            var result = (int)cmd.ExecuteScalar()!;
            if (result != i) throw new Exception($"Expected {i}, got {result}");
        }
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Multiple parallel connections
Console.WriteLine("Test 2: Multiple parallel connections");
var tasks = new List<Task>();
var results = new int[20];
for (int i = 0; i < 20; i++)
{
    int index = i;
    tasks.Add(Task.Run(async () =>
    {
        await using var connection = new NpgsqlConnection(connectionString);
        await connection.OpenAsync();
        await using var cmd = new NpgsqlCommand($"SELECT {index} as val", connection);
        results[index] = (int)(await cmd.ExecuteScalarAsync())!;
    }));
}
Task.WaitAll(tasks.ToArray());
for (int i = 0; i < 20; i++)
{
    if (results[i] != i) throw new Exception($"Parallel test failed: expected {i}, got {results[i]}");
}
Console.WriteLine("Test 2 complete");

// Test 3: Connection reuse with different queries
Console.WriteLine("Test 3: Connection reuse");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    // Execute many different queries on same connection
    for (int i = 0; i < 50; i++)
    {
        using (var cmd = new NpgsqlCommand($"SELECT {i} * 2 as doubled", connection))
        {
            var result = (int)cmd.ExecuteScalar()!;
            if (result != i * 2) throw new Exception($"Expected {i * 2}, got {result}");
        }
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Interleaved connections with shared table
Console.WriteLine("Test 4: Interleaved connections");
using (var setupConn = new NpgsqlConnection(connectionString))
{
    setupConn.Open();
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_interleaved; CREATE TABLE test_interleaved(id serial, session_id int, val int)", setupConn))
    {
        cmd.ExecuteNonQuery();
    }
}

var conn1 = new NpgsqlConnection(connectionString);
var conn2 = new NpgsqlConnection(connectionString);
conn1.Open();
conn2.Open();

// Interleave operations
for (int i = 0; i < 10; i++)
{
    using (var cmd = new NpgsqlCommand("INSERT INTO test_interleaved(session_id, val) VALUES(1, @val)", conn1))
    {
        cmd.Parameters.AddWithValue("val", i);
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("INSERT INTO test_interleaved(session_id, val) VALUES(2, @val)", conn2))
    {
        cmd.Parameters.AddWithValue("val", i * 10);
        cmd.ExecuteNonQuery();
    }
}

// Verify results
using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_interleaved", conn1))
{
    var count = (long)cmd.ExecuteScalar()!;
    if (count != 20) throw new Exception($"Expected 20 rows, got {count}");
}

using (var cmd = new NpgsqlCommand("SELECT SUM(val) FROM test_interleaved WHERE session_id = 1", conn1))
{
    var sum = (long)cmd.ExecuteScalar()!;
    if (sum != 45) throw new Exception($"Expected sum 45 for session 1, got {sum}");
}

using (var cmd = new NpgsqlCommand("SELECT SUM(val) FROM test_interleaved WHERE session_id = 2", conn2))
{
    var sum = (long)cmd.ExecuteScalar()!;
    if (sum != 450) throw new Exception($"Expected sum 450 for session 2, got {sum}");
}

conn1.Close();
conn2.Close();
Console.WriteLine("Test 4 complete");

// Test 5: Prepared statements across multiple connections
Console.WriteLine("Test 5: Prepared statements across connections");
var prepTasks = new List<Task>();
for (int connIdx = 0; connIdx < 5; connIdx++)
{
    int idx = connIdx;
    prepTasks.Add(Task.Run(async () =>
    {
        await using var connection = new NpgsqlConnection(connectionString);
        await connection.OpenAsync();
        
        await using var cmd = new NpgsqlCommand("SELECT @a::int + @b::int", connection);
        cmd.Parameters.Add("a", NpgsqlDbType.Integer);
        cmd.Parameters.Add("b", NpgsqlDbType.Integer);
        cmd.Prepare();
        
        for (int i = 0; i < 20; i++)
        {
            cmd.Parameters["a"].Value = idx;
            cmd.Parameters["b"].Value = i;
            var result = (int)(await cmd.ExecuteScalarAsync())!;
            if (result != idx + i) throw new Exception($"Conn {idx}: expected {idx + i}, got {result}");
        }
    }));
}
Task.WaitAll(prepTasks.ToArray());
Console.WriteLine("Test 5 complete");

// Test 6: Rapid connect/disconnect cycles
Console.WriteLine("Test 6: Rapid connect/disconnect");
for (int i = 0; i < 30; i++)
{
    using (var connection = new NpgsqlConnection(connectionString))
    {
        connection.Open();
        using (var cmd = new NpgsqlCommand("SELECT 1", connection))
        {
            cmd.ExecuteScalar();
        }
        connection.Close();
    }
}
Console.WriteLine("Test 6 complete");

// Test 7: Long-running connection with periodic queries
Console.WriteLine("Test 7: Long-running connection");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    for (int i = 0; i < 20; i++)
    {
        using (var cmd = new NpgsqlCommand("SELECT pg_sleep(0.01), @i::int as iteration", connection))
        {
            cmd.Parameters.AddWithValue("i", i);
            using (var reader = cmd.ExecuteReader())
            {
                reader.Read();
                var iteration = reader.GetInt32(1);
                if (iteration != i) throw new Exception($"Expected iteration {i}, got {iteration}");
            }
        }
    }
}
Console.WriteLine("Test 7 complete");

// Test 8: Concurrent transactions on different connections
Console.WriteLine("Test 8: Concurrent transactions");
using (var setupConn = new NpgsqlConnection(connectionString))
{
    setupConn.Open();
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_concurrent_tx; CREATE TABLE test_concurrent_tx(id int PRIMARY KEY, val int)", setupConn))
    {
        cmd.ExecuteNonQuery();
    }
}

var txTasks = new List<Task>();
for (int txIdx = 0; txIdx < 5; txIdx++)
{
    int idx = txIdx;
    txTasks.Add(Task.Run(async () =>
    {
        await using var connection = new NpgsqlConnection(connectionString);
        await connection.OpenAsync();
        
        await using var tx = await connection.BeginTransactionAsync();
        
        await using (var cmd = new NpgsqlCommand($"INSERT INTO test_concurrent_tx(id, val) VALUES({idx * 100}, {idx})", connection, tx))
        {
            await cmd.ExecuteNonQueryAsync();
        }
        
        await using (var cmd = new NpgsqlCommand($"INSERT INTO test_concurrent_tx(id, val) VALUES({idx * 100 + 1}, {idx + 10})", connection, tx))
        {
            await cmd.ExecuteNonQueryAsync();
        }
        
        await tx.CommitAsync();
    }));
}
Task.WaitAll(txTasks.ToArray());

using (var verifyConn = new NpgsqlConnection(connectionString))
{
    verifyConn.Open();
    using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_concurrent_tx", verifyConn))
    {
        var count = (long)cmd.ExecuteScalar()!;
        if (count != 10) throw new Exception($"Expected 10 rows from concurrent transactions, got {count}");
    }
}
Console.WriteLine("Test 8 complete");

Console.WriteLine("multi_session complete");
