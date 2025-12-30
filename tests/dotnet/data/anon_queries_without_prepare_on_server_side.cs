using Npgsql;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;SSLMode=Disable";

await using var connection = new NpgsqlConnection(connectionString);
await using var batch = connection.CreateBatch();

for (int i = 0; i < 1024; i++)
{
    var command = batch.CreateBatchCommand();
    command.CommandText = "select 1::bigint";
    batch.BatchCommands.Add(command);
}

await connection.OpenAsync();
try
{
    var reader = await batch.ExecuteReaderAsync();
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

Console.WriteLine("anon_queries_without_prepare_on_server_side complete");