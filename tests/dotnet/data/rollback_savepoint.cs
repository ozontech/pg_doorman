using Npgsql;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;Username=example_user_1;Password=test;";

Console.WriteLine("Test: Savepoint rollback with NpgsqlBatch");

try
{
    var builder = new NpgsqlConnectionStringBuilder(connectionString);
    builder.Pooling = false;
    connectionString = builder.ConnectionString;

    await using var connection = new NpgsqlConnection(connectionString);
    await connection.OpenAsync();
    Console.WriteLine("Connection opened");

    await using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_savepoint_dotnet; CREATE TABLE test_savepoint_dotnet (id serial PRIMARY KEY, value int)", connection))
    {
        await cmd.ExecuteNonQueryAsync();
    }

    await using var transaction = await connection.BeginTransactionAsync();
    Console.WriteLine("Transaction started");

    await using (var cmd = new NpgsqlCommand("INSERT INTO test_savepoint_dotnet (value) VALUES (1)", connection, transaction))
    {
        await cmd.ExecuteNonQueryAsync();
    }

    await using (var cmd = new NpgsqlCommand("SAVEPOINT sp", connection, transaction))
    {
        await cmd.ExecuteNonQueryAsync();
    }
    Console.WriteLine("Savepoint sp created");

    // Start a batch that will fail
    await using var batch = connection.CreateBatch();
    
    var cmd1 = batch.CreateBatchCommand();
    cmd1.CommandText = "INSERT INTO test_savepoint_dotnet (value) VALUES (2)";
    batch.BatchCommands.Add(cmd1);

    var cmd2 = batch.CreateBatchCommand();
    cmd2.CommandText = "SELECT * FROM unknown_table"; // This will fail
    batch.BatchCommands.Add(cmd2);

    try
    {
        await batch.ExecuteNonQueryAsync();
    }
    catch (PostgresException ex)
    {
        Console.WriteLine($"Caught expected exception: {ex.Message}");
    }

    Console.WriteLine("Rolling back to savepoint sp...");
    await using (var cmd = new NpgsqlCommand("ROLLBACK TO SAVEPOINT sp", connection, transaction))
    {
        await cmd.ExecuteNonQueryAsync();
    }

    // Verify data
    await using (var cmd = new NpgsqlCommand("SELECT count(*) FROM test_savepoint_dotnet", connection, transaction))
    {
        var count = await cmd.ExecuteScalarAsync();
        Console.WriteLine($"Count after rollback to savepoint: {count}");
        if (Convert.ToInt32(count) != 1)
        {
            throw new Exception($"Expected count 1, but got {count}");
        }
    }

    await transaction.CommitAsync();
    Console.WriteLine("Transaction committed successfully");

    // Final verify
    await using (var cmd = new NpgsqlCommand("SELECT count(*) FROM test_savepoint_dotnet", connection))
    {
        var count = await cmd.ExecuteScalarAsync();
        Console.WriteLine($"Final count: {count}");
        if (Convert.ToInt32(count) != 1)
        {
            throw new Exception($"Final expected count 1, but got {count}");
        }
    }

    Console.WriteLine("✓ .NET Savepoint rollback test passed");
}
catch (Exception ex)
{
    Console.WriteLine($"Test FAILED: {ex.Message}");
    Console.WriteLine(ex.StackTrace);
    Environment.Exit(1);
}
