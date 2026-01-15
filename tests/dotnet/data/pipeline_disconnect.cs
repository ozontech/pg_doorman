using Npgsql;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;Username=example_user_1;Password=test;";

// Disable connection pooling in Npgsql to ensure we go through pg_doorman's pool
connectionString += "Pooling=false;";

Console.WriteLine("Test: Pipeline disconnect - client A crashes during batch, client B gets same connection");

try
{
    await Execute(true);
}
catch (Exception ex)
{
    Console.WriteLine($"Client A: Exception caught - {ex.Message}");
    NpgsqlConnection.ClearAllPools();
}

Console.WriteLine("Client A: Disconnected, now trying Client B...");

// Small delay to let pg_doorman detect the disconnect
await Task.Delay(500);

await Execute(false);

Console.WriteLine("pipeline_disconnect complete");

async Task Execute(bool throwException)
{
    string clientName = throwException ? "A" : "B";
    Console.WriteLine($"Client {clientName}: Starting batch execution...");
    
    await using var connection = new NpgsqlConnection(connectionString);
    
    // Create batch with multiple commands - they are sent without Sync, synced at the end
    await using var batch = connection.CreateBatch();
    
    // Add multiple commands that return data
    for (int i = 0; i < 10; i++)
    {
        var command = batch.CreateBatchCommand();
        // Query that returns some data - using generate_series to create rows
        command.CommandText = $"SELECT {i} as batch_num, generate_series(1, 1000) as num, repeat('X', 1024) as data";
        batch.BatchCommands.Add(command);
    }
    
    await connection.OpenAsync();
    Console.WriteLine($"Client {clientName}: Connection opened");
    
    try
    {
        // PrepareAsync sends Parse messages for all commands
        await batch.PrepareAsync();
        Console.WriteLine($"Client {clientName}: Batch prepared");
        
        // ExecuteReaderAsync sends Bind/Execute for all commands, then Sync
        await using var reader = await batch.ExecuteReaderAsync();
        Console.WriteLine($"Client {clientName}: Reader created");
        
        int resultSetCount = 0;
        int totalRowsRead = 0;
        
        do
        {
            resultSetCount++;
            while (await reader.ReadAsync())
            {
                totalRowsRead++;
                
                if (throwException && totalRowsRead == 5)
                {
                    // Simulate client crash after reading only 5 rows
                    // This leaves the server with unread data
                    Console.WriteLine($"Client {clientName}: Read {totalRowsRead} rows, now throwing exception (simulating crash)...");
                    throw new Exception("Simulated client crash!");
                }
                
                // Read the data
                var batchNum = reader.GetInt32(0);
                var num = reader.GetInt32(1);
                var data = reader.GetString(2);
            }
        }
        while (await reader.NextResultAsync());
        
        Console.WriteLine($"Client {clientName}: Read {resultSetCount} result sets, {totalRowsRead} total rows");
        
        if (!throwException)
        {
            // Verify we got all expected data
            if (resultSetCount != 10)
            {
                throw new Exception($"Expected 10 result sets, got {resultSetCount}");
            }
            if (totalRowsRead != 10000)
            {
                throw new Exception($"Expected 10000 rows, got {totalRowsRead}");
            }
            Console.WriteLine($"Client {clientName}: All results correct!");
        }
    }
    finally
    {
        await connection.CloseAsync();
        Console.WriteLine($"Client {clientName}: Connection closed");
    }
}
