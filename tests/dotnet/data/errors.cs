using Npgsql;
using NpgsqlTypes;
using System;

// Use DATABASE_URL environment variable if set, otherwise use default
string connectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;User Id=example_user_1;Password=test;";

// Test 1: Syntax error handling
Console.WriteLine("Test 1: Syntax error");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    try
    {
        using (var cmd = new NpgsqlCommand("SELEC * FROM nonexistent", connection))
        {
            cmd.ExecuteNonQuery();
        }
        throw new Exception("Expected syntax error");
    }
    catch (PostgresException ex)
    {
        if (!ex.Message.Contains("syntax error"))
        {
            throw new Exception($"Expected syntax error, got: {ex.Message}");
        }
        Console.WriteLine($"Caught expected syntax error: {ex.SqlState}");
    }
    
    // Connection should still be usable
    using (var cmd = new NpgsqlCommand("SELECT 1", connection))
    {
        var result = cmd.ExecuteScalar();
        if ((int)result! != 1) throw new Exception("Connection broken after error");
    }
}
Console.WriteLine("Test 1 complete");

// Test 2: Table not found error
Console.WriteLine("Test 2: Table not found");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    try
    {
        using (var cmd = new NpgsqlCommand("SELECT * FROM table_that_does_not_exist_12345", connection))
        {
            cmd.ExecuteReader();
        }
        throw new Exception("Expected relation not found error");
    }
    catch (PostgresException ex)
    {
        if (ex.SqlState != "42P01") // undefined_table
        {
            throw new Exception($"Expected undefined_table error (42P01), got: {ex.SqlState}");
        }
        Console.WriteLine($"Caught expected error: {ex.SqlState}");
    }
    
    // Connection should still be usable
    using (var cmd = new NpgsqlCommand("SELECT 2", connection))
    {
        var result = cmd.ExecuteScalar();
        if ((int)result! != 2) throw new Exception("Connection broken after error");
    }
}
Console.WriteLine("Test 2 complete");

// Test 3: Constraint violation (unique)
Console.WriteLine("Test 3: Unique constraint violation");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_unique; CREATE TABLE test_unique(id int PRIMARY KEY)", connection))
    {
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("INSERT INTO test_unique(id) VALUES(1)", connection))
    {
        cmd.ExecuteNonQuery();
    }
    
    try
    {
        using (var cmd = new NpgsqlCommand("INSERT INTO test_unique(id) VALUES(1)", connection))
        {
            cmd.ExecuteNonQuery();
        }
        throw new Exception("Expected unique violation error");
    }
    catch (PostgresException ex)
    {
        if (ex.SqlState != "23505") // unique_violation
        {
            throw new Exception($"Expected unique_violation error (23505), got: {ex.SqlState}");
        }
        Console.WriteLine($"Caught expected error: {ex.SqlState}");
    }
    
    // Connection should still be usable
    using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_unique", connection))
    {
        var count = (long)cmd.ExecuteScalar()!;
        if (count != 1) throw new Exception($"Expected 1 row, got {count}");
    }
}
Console.WriteLine("Test 3 complete");

// Test 4: Division by zero
Console.WriteLine("Test 4: Division by zero");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    try
    {
        using (var cmd = new NpgsqlCommand("SELECT 1/0", connection))
        {
            cmd.ExecuteScalar();
        }
        throw new Exception("Expected division by zero error");
    }
    catch (PostgresException ex)
    {
        if (ex.SqlState != "22012") // division_by_zero
        {
            throw new Exception($"Expected division_by_zero error (22012), got: {ex.SqlState}");
        }
        Console.WriteLine($"Caught expected error: {ex.SqlState}");
    }
    
    // Connection should still be usable
    using (var cmd = new NpgsqlCommand("SELECT 3", connection))
    {
        var result = cmd.ExecuteScalar();
        if ((int)result! != 3) throw new Exception("Connection broken after error");
    }
}
Console.WriteLine("Test 4 complete");

// Test 5: Error in prepared statement
Console.WriteLine("Test 5: Error in prepared statement");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    using (var cmd = new NpgsqlCommand("DROP TABLE IF EXISTS test_prep_err; CREATE TABLE test_prep_err(id int NOT NULL)", connection))
    {
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("INSERT INTO test_prep_err(id) VALUES(@val)", connection))
    {
        cmd.Parameters.Add("val", NpgsqlDbType.Integer);
        cmd.Prepare();
        
        // First insert should succeed
        cmd.Parameters["val"].Value = 1;
        cmd.ExecuteNonQuery();
        
        // NULL should fail (NOT NULL constraint)
        try
        {
            cmd.Parameters["val"].Value = DBNull.Value;
            cmd.ExecuteNonQuery();
            throw new Exception("Expected NOT NULL violation");
        }
        catch (PostgresException ex)
        {
            if (ex.SqlState != "23502") // not_null_violation
            {
                throw new Exception($"Expected not_null_violation error (23502), got: {ex.SqlState}");
            }
            Console.WriteLine($"Caught expected error: {ex.SqlState}");
        }
        
        // Prepared statement should still work
        cmd.Parameters["val"].Value = 2;
        cmd.ExecuteNonQuery();
    }
    
    using (var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM test_prep_err", connection))
    {
        var count = (long)cmd.ExecuteScalar()!;
        if (count != 2) throw new Exception($"Expected 2 rows, got {count}");
    }
}
Console.WriteLine("Test 5 complete");

// Test 6: Multiple errors in sequence
Console.WriteLine("Test 6: Multiple errors in sequence");
using (var connection = new NpgsqlConnection(connectionString))
{
    connection.Open();
    
    for (int i = 0; i < 5; i++)
    {
        try
        {
            using (var cmd = new NpgsqlCommand($"SELECT * FROM nonexistent_table_{i}", connection))
            {
                cmd.ExecuteReader();
            }
        }
        catch (PostgresException)
        {
            // Expected
        }
        
        // Verify connection still works
        using (var cmd = new NpgsqlCommand($"SELECT {i}", connection))
        {
            var result = cmd.ExecuteScalar();
            if ((int)result! != i) throw new Exception($"Connection broken after error {i}");
        }
    }
}
Console.WriteLine("Test 6 complete");

Console.WriteLine("errors complete");
