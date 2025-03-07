using Npgsql;
using System;

string connectionString = "Host=localhost;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";
using NpgsqlConnection connection = new NpgsqlConnection(connectionString);
connection.Open();

await using (NpgsqlCommand cmd = new NpgsqlCommand("select 1 as test; select 2 as test;", connection))
{
    var response = await cmd.ExecuteNonQueryAsync();
}
;

await using (NpgsqlCommand cmd = new NpgsqlCommand("select 1 as test; select 2 as test;", connection))
{
    var response = await cmd.ExecuteNonQueryAsync();
}
;