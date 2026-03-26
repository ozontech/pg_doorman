using System.Data;
using System.Net.Sockets;
using System.Reflection;
using Npgsql;

// Exact reproduction of the reported bug
// Use DATABASE_URL environment variable if set, otherwise use default
string baseConnectionString = Environment.GetEnvironmentVariable("DATABASE_URL")
    ?? "Host=127.0.0.1;Port=6433;Database=example_db;Username=example_user_1;Password=test;";

// Npgsql pool with Maximum Pool Size=1 — forces connection reuse on Npgsql side
// Timeout=0 — no connection timeout
string connectionString = baseConnectionString + "Timeout=0;Maximum Pool Size=1;";

Console.WriteLine("Test: Pipeline cancel disconnect - kill socket during 4MB transfer");

// Client A: send query with ~4MB parameter, read result, kill socket mid-transfer
try
{
    await RunWithPhysicalBreakConnection(connectionString);
}
catch
{
    // ignore
}
Console.WriteLine("Client A: Exception caught");

// Client B: reuse the same connection — must work cleanly
try
{
    await Run(connectionString);
    Console.WriteLine("Client B: Query completed successfully");
}
catch (Exception e)
{
    if (e.ToString().Contains("Please file a bug"))
    {
        Console.WriteLine("Bug detected: " + e.Message);
    }
    else
    {
        Console.WriteLine("Client B: Error - " + e.Message);
    }
}

Console.WriteLine("pipeline_cancel_disconnect complete");

async Task RunWithPhysicalBreakConnection(string connStr)
{
    await using var connection = new NpgsqlConnection(connStr);
    await using var cmd = connection.CreateCommand();
    cmd.CommandText = "SELECT @payload";
    // ~4MB text parameter
    cmd.Parameters.Add(new NpgsqlParameter("payload", string.Join("", Enumerable.Repeat("0", 1_000_000))));
    await connection.OpenAsync();
    await using var reader = await cmd.ExecuteReaderAsync(CommandBehavior.SequentialAccess);
    while (await reader.ReadAsync())
    {
        _ = reader.GetString(0);
        KillTransport(connection);
    }
    await connection.CloseAsync();
}

async Task Run(string connStr)
{
    await using var connection = new NpgsqlConnection(connStr);
    await using var cmd = connection.CreateCommand();
    cmd.CommandText = "SELECT @payload";
    cmd.Parameters.Add(new NpgsqlParameter("payload", string.Join("", Enumerable.Repeat("0", 1_000_000))));
    await connection.OpenAsync();
    await using var reader = await cmd.ExecuteReaderAsync(CommandBehavior.SequentialAccess);
    while (await reader.ReadAsync())
    {
        _ = reader.GetString(0);
    }
    await connection.CloseAsync();
}

void KillTransport(NpgsqlConnection connection, bool abortive = true)
{
    var connectorProp = typeof(NpgsqlConnection).GetProperty(
                            "Connector",
                            BindingFlags.Instance | BindingFlags.NonPublic)
                        ?? throw new MissingMemberException("NpgsqlConnection.Connector not found.");

    var connector = connectorProp.GetValue(connection)
                    ?? throw new InvalidOperationException("Connection has no bound connector.");

    var t = connector.GetType();

    var socketField = t.GetField("_socket", BindingFlags.Instance | BindingFlags.NonPublic);
    var streamField = t.GetField("_stream", BindingFlags.Instance | BindingFlags.NonPublic);
    var baseStreamField = t.GetField("_baseStream", BindingFlags.Instance | BindingFlags.NonPublic);

    if (socketField?.GetValue(connector) is Socket socket && abortive)
        socket.LingerState = new LingerOption(enable: true, seconds: 0);

    try { (streamField?.GetValue(connector) as IDisposable)?.Dispose(); } catch { }
    try { (baseStreamField?.GetValue(connector) as IDisposable)?.Dispose(); } catch { }
    try { (socketField?.GetValue(connector) as IDisposable)?.Dispose(); } catch { }

    NpgsqlConnection.ClearPool(connection);
}
