using Npgsql;
using NpgsqlTypes;
using System;
using System.Threading.Tasks;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Prepared statements with different data types
Console.WriteLine("Test 1: Different data types");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    // Setup table
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_prepared_types; CREATE TABLE test_prepared_types(id serial primary key, int_val int, text_val text, bool_val boolean, float_val float8, ts_val timestamp)", connection))
    {
        cmd.ExecuteNonQuery();
    }
    
    // Prepared insert with multiple types
    using (var cmd = new NpgsqlCommand("INSERT INTO test_prepared_types(int_val, text_val, bool_val, float_val, ts_val) VALUES(@int, @text, @bool, @float, @ts)", connection))
    {
        cmd.Parameters.Add("int", NpgsqlDbType.Integer);
        cmd.Parameters.Add("text", NpgsqlDbType.Text);
        cmd.Parameters.Add("bool", NpgsqlDbType.Boolean);
        cmd.Parameters.Add("float", NpgsqlDbType.Double);
        cmd.Parameters.Add("ts", NpgsqlDbType.Timestamp);
        cmd.Prepare();
        
        for (int i = 0; i < 10; i++)
        {
            cmd.Parameters["int"].Value = i;
            cmd.Parameters["text"].Value = $"text_{i}";
            cmd.Parameters["bool"].Value = i % 2 == 0;
            cmd.Parameters["float"].Value = i * 1.5;
            cmd.Parameters["ts"].Value = DateTime.Now.AddDays(i);
            cmd.ExecuteNonQuery();
        }
    }
    
    // Prepared select
    using (var cmd = new NpgsqlCommand("SELECT * FROM test_prepared_types WHERE int_val > @min", connection))
    {
        cmd.Parameters.Add("min", NpgsqlDbType.Integer).Value = 5;
        cmd.Prepare();
        
        using (var reader = cmd.ExecuteReader())
        {
            int count = 0;
            while (reader.Read())
            {
                count++;
            }
            if (count != 4) throw new Exception($"Expected 4 rows, got {count}");
        }
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Reuse prepared statement across multiple executions
Console.WriteLine("Test 2: Reuse prepared statement");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    using (var cmd = new NpgsqlCommand("SELECT @val::int * 2 as result", connection))
    {
        cmd.Parameters.Add("val", NpgsqlDbType.Integer);
        cmd.Prepare();
        
        for (int i = 0; i < 100; i++)
        {
            cmd.Parameters["val"].Value = i;
            var result = (int)cmd.ExecuteScalar()!;
            if (result != i * 2) throw new Exception($"Expected {i * 2}, got {result}");
        }
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Multiple prepared statements in same connection
Console.WriteLine("Test 3: Multiple prepared statements");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    var cmd1 = new NpgsqlCommand("SELECT @a::int + @b::int", connection);
    cmd1.Parameters.Add("a", NpgsqlDbType.Integer);
    cmd1.Parameters.Add("b", NpgsqlDbType.Integer);
    cmd1.Prepare();
    
    var cmd2 = new NpgsqlCommand("SELECT @x::int * @y::int", connection);
    cmd2.Parameters.Add("x", NpgsqlDbType.Integer);
    cmd2.Parameters.Add("y", NpgsqlDbType.Integer);
    cmd2.Prepare();
    
    for (int i = 0; i < 50; i++)
    {
        cmd1.Parameters["a"].Value = i;
        cmd1.Parameters["b"].Value = i + 1;
        var sum = (int)cmd1.ExecuteScalar()!;
        
        cmd2.Parameters["x"].Value = i;
        cmd2.Parameters["y"].Value = 2;
        var product = (int)cmd2.ExecuteScalar()!;
        
        if (sum != i + i + 1) throw new Exception($"Sum mismatch: expected {i + i + 1}, got {sum}");
        if (product != i * 2) throw new Exception($"Product mismatch: expected {i * 2}, got {product}");
    }
    
    cmd1.Dispose();
    cmd2.Dispose();
}
Console.WriteLine("Test 3 complete");

// Test 4: Prepared statement with NULL values
Console.WriteLine("Test 4: NULL values");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_nulls; CREATE TABLE test_nulls(id serial, val int)", connection))
    {
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("INSERT INTO test_nulls(val) VALUES(@val)", connection))
    {
        cmd.Parameters.Add("val", NpgsqlDbType.Integer);
        cmd.Prepare();
        
        // Insert NULL
        cmd.Parameters["val"].Value = DBNull.Value;
        cmd.ExecuteNonQuery();
        
        // Insert regular value
        cmd.Parameters["val"].Value = 42;
        cmd.ExecuteNonQuery();
        
        // Insert NULL again
        cmd.Parameters["val"].Value = DBNull.Value;
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_nulls WHERE val IS NULL", connection))
    {
        var nullCount = (long)cmd.ExecuteScalar()!;
        if (nullCount != 2) throw new Exception($"Expected 2 NULL rows, got {nullCount}");
    }
}
Console.WriteLine("Test 4 complete");

Console.WriteLine("prepared_advanced complete");
