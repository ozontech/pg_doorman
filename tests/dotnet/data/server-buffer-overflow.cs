using Npgsql;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;SSLMode=Disable";

await using var connection = new NpgsqlConnection(connectionString);
await using var selectionBatch = connection.CreateBatch();

for (int i = 0; i < 2; i++)
{
    var sCommand = selectionBatch.CreateBatchCommand();
    // Response data should be greater than Server.buffer.len()
    sCommand.CommandText = "select * from generate_series(1, 10000)";
    selectionBatch.BatchCommands.Add(sCommand);
}
await connection.OpenAsync();
try
{
    var reader = await selectionBatch.ExecuteReaderAsync();
    do
    {
        while (await reader.ReadAsync())
        {
            _ = reader.GetInt64(0);
        }
    } while (await reader.NextResultAsync());
}
finally
{
    await connection.CloseAsync();
}

Console.WriteLine("server-buffer-overflow complete");