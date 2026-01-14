using Npgsql;
using NpgsqlTypes;
using System;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// This test specifically covers the Describe flow when Parse is skipped due to caching.
// When pg_doorman caches a prepared statement and skips sending Parse to the server,
// it must still insert ParseComplete before ParameterDescription in the response.
//
// Protocol flow:
// Client sends: Parse + Describe + Sync
// Server responds: ParseComplete + ParameterDescription + RowDescription + ReadyForQuery
//
// When Parse is cached (skipped):
// Client sends: Describe + Sync (Parse was skipped by pg_doorman)
// Server responds: ParameterDescription + RowDescription + ReadyForQuery
// pg_doorman must insert: ParseComplete + ParameterDescription + RowDescription + ReadyForQuery

Console.WriteLine("Test: Describe flow with cached prepared statement");

// Test 1: Basic Describe flow - first Prepare sends Parse, second reuses cache
Console.WriteLine("Test 1: Basic cached Describe flow");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Create test table
    using (var cmd = new NpgsqlCommand(@"
        DROP TABLE IF EXISTS test_describe_cached;
        CREATE TABLE test_describe_cached(id serial primary key, value text)", connection))
    {
        cmd.ExecuteNonQuery();
    }

    // First Prepare - Parse is sent to server
    using (var cmd1 = new NpgsqlCommand("SELECT * FROM test_describe_cached WHERE id = @id", connection))
    {
        cmd1.Parameters.Add("id", NpgsqlDbType.Integer);
        cmd1.Prepare(); // Parse + Describe + Sync -> ParseComplete + ParameterDescription + RowDescription + ReadyForQuery
        
        cmd1.Parameters["id"].Value = 1;
        using (var reader = cmd1.ExecuteReader())
        {
            // No rows expected, just verify it works
        }
    }

    // Second Prepare with SAME query - Parse should be cached/skipped
    // This is the critical test: pg_doorman must insert ParseComplete before ParameterDescription
    using (var cmd2 = new NpgsqlCommand("SELECT * FROM test_describe_cached WHERE id = @id", connection))
    {
        cmd2.Parameters.Add("id", NpgsqlDbType.Integer);
        cmd2.Prepare(); // Parse cached -> Describe + Sync, but client expects ParseComplete first!
        
        cmd2.Parameters["id"].Value = 1;
        using (var reader = cmd2.ExecuteReader())
        {
            // No rows expected
        }
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Multiple cached Prepare calls in sequence
Console.WriteLine("Test 2: Multiple cached Prepare calls");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    string query = "SELECT * FROM test_describe_cached WHERE id > @minId ORDER BY id";
    
    for (int i = 0; i < 10; i++)
    {
        using (var cmd = new NpgsqlCommand(query, connection))
        {
            cmd.Parameters.Add("minId", NpgsqlDbType.Integer);
            cmd.Prepare(); // First iteration: Parse sent. Subsequent: Parse cached.
            
            cmd.Parameters["minId"].Value = 0;
            using (var reader = cmd.ExecuteReader())
            {
                // Just verify no protocol errors
            }
        }
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Interleaved Prepare of different queries (cache should work per-query)
Console.WriteLine("Test 3: Interleaved different queries");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Insert some test data
    using (var cmd = new NpgsqlCommand("INSERT INTO test_describe_cached(value) VALUES('test1'), ('test2'), ('test3')", connection))
    {
        cmd.ExecuteNonQuery();
    }

    string queryA = "SELECT id FROM test_describe_cached WHERE id = @id";
    string queryB = "SELECT value FROM test_describe_cached WHERE id = @id";
    string queryC = "SELECT id, value FROM test_describe_cached WHERE id = @id";

    for (int round = 0; round < 5; round++)
    {
        // Query A
        using (var cmd = new NpgsqlCommand(queryA, connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Prepare();
            cmd.Parameters["id"].Value = 1;
            using (var reader = cmd.ExecuteReader())
            {
                if (!reader.Read()) throw new Exception($"Round {round} A: No row");
                if (reader.GetInt32(0) != 1) throw new Exception($"Round {round} A: Wrong id");
            }
        }

        // Query B
        using (var cmd = new NpgsqlCommand(queryB, connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Prepare();
            cmd.Parameters["id"].Value = 2;
            using (var reader = cmd.ExecuteReader())
            {
                if (!reader.Read()) throw new Exception($"Round {round} B: No row");
                if (reader.GetString(0) != "test2") throw new Exception($"Round {round} B: Wrong value");
            }
        }

        // Query C
        using (var cmd = new NpgsqlCommand(queryC, connection))
        {
            cmd.Parameters.Add("id", NpgsqlDbType.Integer);
            cmd.Prepare();
            cmd.Parameters["id"].Value = 3;
            using (var reader = cmd.ExecuteReader())
            {
                if (!reader.Read()) throw new Exception($"Round {round} C: No row");
                if (reader.GetInt32(0) != 3) throw new Exception($"Round {round} C: Wrong id");
                if (reader.GetString(1) != "test3") throw new Exception($"Round {round} C: Wrong value");
            }
        }
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Prepare without Execute (pure Describe flow)
Console.WriteLine("Test 4: Prepare without immediate Execute");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();

    // Prepare multiple statements without executing them
    var cmds = new List<NpgsqlCommand>();
    for (int i = 0; i < 5; i++)
    {
        var cmd = new NpgsqlCommand($"SELECT * FROM test_describe_cached WHERE id = @id", connection);
        cmd.Parameters.Add("id", NpgsqlDbType.Integer);
        cmd.Prepare(); // This sends Parse + Describe + Sync
        cmds.Add(cmd);
    }

    // Now execute them all
    foreach (var cmd in cmds)
    {
        cmd.Parameters["id"].Value = 1;
        using (var reader = cmd.ExecuteReader())
        {
            // Just verify no errors
        }
        cmd.Dispose();
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Prepare on new connection (tests cross-connection caching)
Console.WriteLine("Test 5: Prepare across multiple connections");
{
    string query = "SELECT * FROM test_describe_cached WHERE id = @id";
    
    for (int connNum = 0; connNum < 5; connNum++)
    {
        using (var connection = new NpgsqlConnection(connectionString))
        {
            connection.Open();
            
            using (var cmd = new NpgsqlCommand(query, connection))
            {
                cmd.Parameters.Add("id", NpgsqlDbType.Integer);
                cmd.Prepare(); // Each connection may get different server from pool
                
                cmd.Parameters["id"].Value = 1;
                using (var reader = cmd.ExecuteReader())
                {
                    if (!reader.Read()) throw new Exception($"Conn {connNum}: No row");
                }
            }
        }
    }
}
Console.WriteLine("Test 5 complete");

Console.WriteLine("describe_flow_cached complete");
