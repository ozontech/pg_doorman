using Npgsql;
using NpgsqlTypes;
using System;
using System.Data;
using System.Threading;
using System.Threading.Tasks;

// Репродукция бага с PrepareAsync в NpgsqlBatch


string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

Console.WriteLine("Starting issue_repro_batch_prepare test");

using var connection = new NpgsqlConnection(connectionString);
await connection.OpenAsync();

using (var cmd = new NpgsqlCommand(@"
    DROP TABLE IF EXISTS documents;
    DROP TABLE IF EXISTS outbox;
    CREATE TABLE documents (id bigint primary key, accounted_date date);
    CREATE TABLE outbox (id serial primary key, message_type text, message_data bytea);
    INSERT INTO documents (id, accounted_date) VALUES (1, NULL);
", connection))
{
    await cmd.ExecuteNonQueryAsync();
}

var token = CancellationToken.None;
var accountedDate = DateTime.Today;
var documentId = 1L;

// await using var batch = ctx.CreateBatch();
await using var batch = connection.CreateBatch();

batch.BatchCommands.Add(new NpgsqlBatchCommand
{
    CommandText = "update documents set accounted_date = $1 where id = $2;",
    Parameters =
    {
        new NpgsqlParameter { Value = accountedDate, NpgsqlDbType = NpgsqlDbType.Date },
        new NpgsqlParameter { Value = documentId, NpgsqlDbType = NpgsqlDbType.Bigint }
    }
});

// Имитируем сообщение аутбокса
var outboxMsgType = "DocumentAccountedDateResolved";
var outboxMsgData = new byte[] { 1, 2, 3, 4, 5 };

batch.BatchCommands.Add(new NpgsqlBatchCommand
{
    CommandText = "insert into outbox (message_type, message_data) values ($1, $2);",
    Parameters =
    {
        new NpgsqlParameter { Value = outboxMsgType, NpgsqlDbType = NpgsqlDbType.Text },
        new NpgsqlParameter { Value = outboxMsgData, NpgsqlDbType = NpgsqlDbType.Bytea }
    }
});

Console.WriteLine("Executing batch WITHOUT PrepareAsync...");
await batch.ExecuteNonQueryAsync(token);

await using var batch2 = connection.CreateBatch();
batch2.BatchCommands.Add(new NpgsqlBatchCommand
{
    CommandText = "update documents set accounted_date = $1 where id = $2;",
    Parameters =
    {
        new NpgsqlParameter { Value = accountedDate, NpgsqlDbType = NpgsqlDbType.Date },
        new NpgsqlParameter { Value = documentId, NpgsqlDbType = NpgsqlDbType.Bigint }
    }
});

batch2.BatchCommands.Add(new NpgsqlBatchCommand
{
    CommandText = "insert into outbox (message_type, message_data) values ($1, $2);",
    Parameters =
    {
        new NpgsqlParameter { Value = outboxMsgType, NpgsqlDbType = NpgsqlDbType.Text },
        new NpgsqlParameter { Value = outboxMsgData, NpgsqlDbType = NpgsqlDbType.Bytea }
    }
});

Console.WriteLine("Preparing batch...");
await batch2.PrepareAsync(token);

Console.WriteLine("Executing batch WITH PrepareAsync...");
await batch2.ExecuteNonQueryAsync(token);

using (var checkCmd = new NpgsqlCommand("SELECT accounted_date FROM documents WHERE id = 1", connection))
{
    var result = await checkCmd.ExecuteScalarAsync();
    bool match = false;
    if (result is DateTime dt)
    {
        match = dt.Date == accountedDate.Date;
    }
    else if (result != null && result.GetType().Name == "DateOnly")
    {
        match = result.ToString() == accountedDate.Date.ToShortDateString();
    }
    
    if (result == null || !match)
    {
        throw new Exception($"Verification failed: expected {accountedDate.ToShortDateString()}, got {result} ({result?.GetType()})");
    }
}

using (var checkCmd = new NpgsqlCommand("SELECT count(*) FROM outbox WHERE message_type = $1", connection))
{
    checkCmd.Parameters.AddWithValue(outboxMsgType);
    var count = (long)await checkCmd.ExecuteScalarAsync();
    // We execute batch twice: once without PrepareAsync, once with PrepareAsync
    if (count != 2)
    {
        throw new Exception($"Verification failed: expected 2 outbox messages, got {count}");
    }
}

Console.WriteLine("issue_repro_batch_prepare complete");
