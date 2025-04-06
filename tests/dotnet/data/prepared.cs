using Npgsql;
using NpgsqlTypes;

string connectionString = "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";
using NpgsqlConnection connection = new NpgsqlConnection(connectionString);
connection.Open();

NpgsqlCommand execute = new NpgsqlCommand("drop table if exists test_npgsql; create table test_npgsql(id serial primary key, t int)", connection);
execute.ExecuteNonQuery();

for (int i = 0; i < 10; i++)
{
    string valueName = string.Format("value{0}", i);
    string sqlString = string.Format("/*{0}*/ insert into test_npgsql(t) values(@{1});", i, valueName);
    NpgsqlCommand cmd = new NpgsqlCommand(sqlString, connection);
    var v1 = cmd.Parameters.Add(valueName, NpgsqlDbType.Integer);
    v1.Value = i;
    cmd.Prepare();
    for (int j = 0; j < 10; j++)
    {
        var _ = cmd.ExecuteNonQuery();
    }
}

Console.WriteLine("prepared complete");
