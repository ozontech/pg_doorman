using Npgsql;
using NpgsqlTypes;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;SSLMode=Disable";
using NpgsqlConnection connection = new NpgsqlConnection(connectionString);
connection.Open();

NpgsqlCommand execute = new NpgsqlCommand("drop table if exists test_npgsql_batch; create table test_npgsql_batch(id serial primary key, t int)", connection);
execute.ExecuteNonQuery();
execute.Dispose();

await using var batch1 = new NpgsqlBatch(connection)
{
    BatchCommands =
    {
        new("insert into test_npgsql_batch(t) values (1)"),
        new("select * from test_npgsql_batch"),
        new("insert into test_npgsql_batch(t) values (2)"),
        new("select * from test_npgsql_batch"),
    }
};

await using var reader1 = await batch1.ExecuteReaderAsync();
Console.WriteLine("batch 1 complete");
reader1.Dispose();

await using var batch2 = new NpgsqlBatch(connection)
{
    BatchCommands =
    {
        new("insert into test_npgsql_batch(t) values (1)"),
        new("select * from test_npgsql_batch"),
        new("insert into test_npgsql_batch(t) values (2)"),
        new("select * from test_npgsql_batch"),
    }
};

await using var reader2 = await batch2.ExecuteReaderAsync();
reader2.Dispose();
Console.WriteLine("batch 2 complete");