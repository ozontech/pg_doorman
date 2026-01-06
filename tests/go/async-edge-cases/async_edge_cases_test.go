package doorman_test

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v4"
	"github.com/jackc/pgx/v4/pgxpool"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestMultipleAnonymousPreparedStatements tests that pg_doorman correctly handles
// multiple anonymous prepared statements with the same query.
// This tests hash-based caching for anonymous statements.
func TestMultipleAnonymousPreparedStatements(t *testing.T) {
	t.Log("=== Test: Multiple anonymous prepared statements ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Execute the same query multiple times with different parameters
	// Each execution uses an anonymous prepared statement
	for i := 1; i <= 5; i++ {
		var result int
		err := conn.QueryRow(ctx, "SELECT $1::int", i).Scan(&result)
		require.NoError(t, err)
		assert.Equal(t, i, result)
		t.Logf("  Iteration %d: OK", i)
	}

	t.Log("  ✓ Test passed")
}

// TestPreparedStatementReuse tests reusing explicitly prepared statements.
// This tests caching of named prepared statements.
func TestPreparedStatementReuse(t *testing.T) {
	t.Log("=== Test: Prepared statement reuse ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Prepare a statement
	stmt := "SELECT $1::int + $2::int"
	_, err = conn.Prepare(ctx, "add_stmt", stmt)
	require.NoError(t, err)
	t.Log("  Prepared statement created: OK")

	// Execute the prepared statement multiple times
	for i := 1; i <= 5; i++ {
		var result int
		err := conn.QueryRow(ctx, "add_stmt", i, i*10).Scan(&result)
		require.NoError(t, err)
		expected := i + i*10
		assert.Equal(t, expected, result)
		t.Logf("  Execution %d: %d + %d = %d: OK", i, i, i*10, result)
	}

	// Deallocate the statement
	_, err = conn.Exec(ctx, "DEALLOCATE add_stmt")
	require.NoError(t, err)
	t.Log("  Statement deallocated: OK")

	t.Log("  ✓ Test passed")
}

// TestConcurrentConnections tests multiple concurrent connections.
// This tests connection pooling and prepared statement isolation.
func TestConcurrentConnections(t *testing.T) {
	t.Log("=== Test: Concurrent connections ===")
	ctx := context.Background()

	numWorkers := 5
	queriesPerWorker := 10

	var wg sync.WaitGroup
	errors := make(chan error, numWorkers*queriesPerWorker)

	for worker := 0; worker < numWorkers; worker++ {
		wg.Add(1)
		go func(workerID int) {
			defer wg.Done()

			conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
			if err != nil {
				errors <- fmt.Errorf("worker %d: connect error: %w", workerID, err)
				return
			}
			defer conn.Close(ctx)

			for i := 0; i < queriesPerWorker; i++ {
				var result int
				err := conn.QueryRow(ctx, "SELECT $1::int", workerID*100+i).Scan(&result)
				if err != nil {
					errors <- fmt.Errorf("worker %d, query %d: %w", workerID, i, err)
					return
				}
				if result != workerID*100+i {
					errors <- fmt.Errorf("worker %d, query %d: expected %d, got %d", workerID, i, workerID*100+i, result)
					return
				}
			}
		}(worker)
	}

	wg.Wait()
	close(errors)

	var errList []error
	for err := range errors {
		errList = append(errList, err)
	}

	require.Empty(t, errList, "Concurrent execution errors: %v", errList)
	t.Logf("  %d workers × %d queries: OK", numWorkers, queriesPerWorker)
	t.Log("  ✓ Test passed")
}

// TestLargeResultSet tests handling of large result sets.
// This tests buffering and timeout in async mode.
func TestLargeResultSet(t *testing.T) {
	t.Log("=== Test: Large result set ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Generate 1000 rows
	rows, err := conn.Query(ctx, "SELECT generate_series(1, 1000) as n")
	require.NoError(t, err)
	defer rows.Close()

	count := 0
	for rows.Next() {
		var n int
		err := rows.Scan(&n)
		require.NoError(t, err)
		count++
	}

	require.NoError(t, rows.Err())
	assert.Equal(t, 1000, count)
	t.Logf("  Fetched %d rows: OK", count)
	t.Log("  ✓ Test passed")
}

// TestTransactionWithPreparedStatements tests prepared statements inside transactions.
// This tests BEGIN/COMMIT/ROLLBACK handling.
func TestTransactionWithPreparedStatements(t *testing.T) {
	t.Log("=== Test: Transaction with prepared statements ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Create test table
	_, err = conn.Exec(ctx, "DROP TABLE IF EXISTS test_tx_prepared")
	require.NoError(t, err)
	_, err = conn.Exec(ctx, "CREATE TABLE test_tx_prepared (id int, value text)")
	require.NoError(t, err)
	t.Log("  Table created: OK")

	// Start transaction
	tx, err := conn.Begin(ctx)
	require.NoError(t, err)

	// Insert using prepared statement
	for i := 1; i <= 5; i++ {
		_, err := tx.Exec(ctx, "INSERT INTO test_tx_prepared (id, value) VALUES ($1, $2)", i, fmt.Sprintf("value_%d", i))
		require.NoError(t, err)
	}
	t.Log("  Inserted 5 rows in transaction: OK")

	// Commit transaction
	err = tx.Commit(ctx)
	require.NoError(t, err)
	t.Log("  Transaction committed: OK")

	// Verify data
	var count int
	err = conn.QueryRow(ctx, "SELECT COUNT(*) FROM test_tx_prepared").Scan(&count)
	require.NoError(t, err)
	assert.Equal(t, 5, count)
	t.Logf("  Verified %d rows: OK", count)

	// Cleanup
	_, err = conn.Exec(ctx, "DROP TABLE test_tx_prepared")
	require.NoError(t, err)

	t.Log("  ✓ Test passed")
}

// TestErrorInPreparedStatement tests error handling in prepared statements.
// This tests error recovery and connection state.
func TestErrorInPreparedStatement(t *testing.T) {
	t.Log("=== Test: Error in prepared statement ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Execute query that will cause an error (division by zero)
	var result int
	err = conn.QueryRow(ctx, "SELECT 1 / $1::int", 0).Scan(&result)
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "division by zero")
	t.Log("  Error caught: division by zero: OK")

	// Verify connection is still usable
	err = conn.QueryRow(ctx, "SELECT 42").Scan(&result)
	require.NoError(t, err)
	assert.Equal(t, 42, result)
	t.Log("  Connection recovered: OK")

	t.Log("  ✓ Test passed")
}

// TestMixedSimpleAndExtendedProtocol tests switching between SimpleQuery and Parse/Bind/Execute.
// This tests protocol switching.
func TestMixedSimpleAndExtendedProtocol(t *testing.T) {
	t.Log("=== Test: Mixed simple and extended protocol ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Simple query (no parameters)
	var result1 int
	err = conn.QueryRow(ctx, "SELECT 1").Scan(&result1)
	require.NoError(t, err)
	assert.Equal(t, 1, result1)
	t.Log("  Simple query: OK")

	// Extended query (with parameters)
	var result2 int
	err = conn.QueryRow(ctx, "SELECT $1::int", 2).Scan(&result2)
	require.NoError(t, err)
	assert.Equal(t, 2, result2)
	t.Log("  Extended query: OK")

	// Simple query again
	var result3 int
	err = conn.QueryRow(ctx, "SELECT 3").Scan(&result3)
	require.NoError(t, err)
	assert.Equal(t, 3, result3)
	t.Log("  Simple query again: OK")

	// Extended query again
	var result4 int
	err = conn.QueryRow(ctx, "SELECT $1::int", 4).Scan(&result4)
	require.NoError(t, err)
	assert.Equal(t, 4, result4)
	t.Log("  Extended query again: OK")

	t.Log("  ✓ Test passed")
}

// TestConnectionPool tests connection pooling.
// This tests pool management and connection reuse.
func TestConnectionPool(t *testing.T) {
	t.Log("=== Test: Connection pool ===")
	ctx := context.Background()

	// Create connection pool
	poolConfig, err := pgxpool.ParseConfig(os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	poolConfig.MaxConns = 5
	poolConfig.MinConns = 2

	pool, err := pgxpool.ConnectConfig(ctx, poolConfig)
	require.NoError(t, err)
	defer pool.Close()
	t.Log("  Pool created (max: 5, min: 2): OK")

	// Execute queries concurrently using pool
	numWorkers := 10
	queriesPerWorker := 10

	var wg sync.WaitGroup
	errors := make(chan error, numWorkers*queriesPerWorker)

	for worker := 0; worker < numWorkers; worker++ {
		wg.Add(1)
		go func(workerID int) {
			defer wg.Done()

			for i := 0; i < queriesPerWorker; i++ {
				var result int
				err := pool.QueryRow(ctx, "SELECT $1::int", workerID*100+i).Scan(&result)
				if err != nil {
					errors <- fmt.Errorf("worker %d, query %d: %w", workerID, i, err)
					return
				}
				if result != workerID*100+i {
					errors <- fmt.Errorf("worker %d, query %d: expected %d, got %d", workerID, i, workerID*100+i, result)
					return
				}
			}
		}(worker)
	}

	wg.Wait()
	close(errors)

	var errList []error
	for err := range errors {
		errList = append(errList, err)
	}

	require.Empty(t, errList, "Pool execution errors: %v", errList)
	t.Logf("  %d workers × %d queries with pool: OK", numWorkers, queriesPerWorker)
	t.Log("  ✓ Test passed")
}

// TestCursorIteration tests cursor-based iteration.
// This tests portal handling with FETCH.
func TestCursorIteration(t *testing.T) {
	t.Log("=== Test: Cursor iteration ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Create test table
	_, err = conn.Exec(ctx, "DROP TABLE IF EXISTS test_cursor")
	require.NoError(t, err)
	_, err = conn.Exec(ctx, "CREATE TABLE test_cursor (id int)")
	require.NoError(t, err)

	// Insert 100 rows
	for i := 1; i <= 100; i++ {
		_, err := conn.Exec(ctx, "INSERT INTO test_cursor (id) VALUES ($1)", i)
		require.NoError(t, err)
	}
	t.Log("  Inserted 100 rows: OK")

	// Start transaction for cursor
	tx, err := conn.Begin(ctx)
	require.NoError(t, err)
	defer tx.Rollback(ctx)

	// Declare cursor
	_, err = tx.Exec(ctx, "DECLARE test_cur CURSOR FOR SELECT id FROM test_cursor ORDER BY id")
	require.NoError(t, err)
	t.Log("  Cursor declared: OK")

	// Fetch in batches of 10
	totalFetched := 0
	for {
		rows, err := tx.Query(ctx, "FETCH 10 FROM test_cur")
		require.NoError(t, err)

		count := 0
		for rows.Next() {
			var id int
			err := rows.Scan(&id)
			require.NoError(t, err)
			count++
			totalFetched++
		}
		rows.Close()

		if count == 0 {
			break
		}
		t.Logf("  Fetched batch of %d rows: OK", count)
	}

	assert.Equal(t, 100, totalFetched)
	t.Logf("  Total fetched: %d rows: OK", totalFetched)

	// Close cursor
	_, err = tx.Exec(ctx, "CLOSE test_cur")
	require.NoError(t, err)

	// Cleanup
	err = tx.Commit(ctx)
	require.NoError(t, err)
	_, err = conn.Exec(ctx, "DROP TABLE test_cursor")
	require.NoError(t, err)

	t.Log("  ✓ Test passed")
}

// TestMultipleResultSets tests queries returning multiple result sets.
// This tests handling of multiple DataRow sequences.
func TestMultipleResultSets(t *testing.T) {
	t.Log("=== Test: Multiple result sets ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Execute query with UNION ALL
	rows, err := conn.Query(ctx, "SELECT 1 as a UNION ALL SELECT 2 UNION ALL SELECT 3")
	require.NoError(t, err)
	defer rows.Close()

	results := []int{}
	for rows.Next() {
		var a int
		err := rows.Scan(&a)
		require.NoError(t, err)
		results = append(results, a)
	}

	require.NoError(t, rows.Err())
	assert.Equal(t, []int{1, 2, 3}, results)
	t.Logf("  Fetched %d results: OK", len(results))
	t.Log("  ✓ Test passed")
}

// TestPreparedStatementWithComplexTypes tests prepared statements with complex PostgreSQL types.
// This tests type handling in async mode.
func TestPreparedStatementWithComplexTypes(t *testing.T) {
	t.Log("=== Test: Prepared statement with complex types ===")
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	require.NoError(t, err)
	defer conn.Close(ctx)

	// Array type
	var arrayResult []int32
	err = conn.QueryRow(ctx, "SELECT $1::int[]", []int32{1, 2, 3}).Scan(&arrayResult)
	require.NoError(t, err)
	assert.Equal(t, []int32{1, 2, 3}, arrayResult)
	t.Log("  Array type: OK")

	// JSON type
	jsonData := map[string]string{"key": "value"}
	jsonBytes, err := json.Marshal(jsonData)
	require.NoError(t, err)

	var jsonResult []byte
	err = conn.QueryRow(ctx, "SELECT $1::jsonb", jsonBytes).Scan(&jsonResult)
	require.NoError(t, err)

	var jsonResultMap map[string]string
	err = json.Unmarshal(jsonResult, &jsonResultMap)
	require.NoError(t, err)
	assert.Equal(t, jsonData, jsonResultMap)
	t.Log("  JSON type: OK")

	// Date type
	date := time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)
	var dateResult time.Time
	err = conn.QueryRow(ctx, "SELECT $1::date", date).Scan(&dateResult)
	require.NoError(t, err)
	assert.Equal(t, date.Format("2006-01-02"), dateResult.Format("2006-01-02"))
	t.Log("  Date type: OK")

	// Timestamp type
	ts := time.Date(2024, 1, 1, 12, 0, 0, 0, time.UTC)
	var tsResult time.Time
	err = conn.QueryRow(ctx, "SELECT $1::timestamp", ts).Scan(&tsResult)
	require.NoError(t, err)
	assert.Equal(t, ts.Unix(), tsResult.Unix())
	t.Log("  Timestamp type: OK")

	t.Log("  ✓ Test passed")
}

// TestRapidConnectDisconnect tests rapid connection creation and destruction.
// This tests connection handling under stress.
func TestRapidConnectDisconnect(t *testing.T) {
	t.Log("=== Test: Rapid connect/disconnect ===")
	ctx := context.Background()

	for i := 0; i < 10; i++ {
		conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
		require.NoError(t, err)

		var result int
		err = conn.QueryRow(ctx, "SELECT $1::int", i).Scan(&result)
		require.NoError(t, err)
		assert.Equal(t, i, result)

		err = conn.Close(ctx)
		require.NoError(t, err)
	}

	t.Log("  10 rapid connect/disconnect cycles: OK")
	t.Log("  ✓ Test passed")
}
