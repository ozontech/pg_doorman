"""
Edge case tests for pg_doorman async protocol support with asyncpg.

These tests are designed to stress-test pg_doorman's async protocol implementation
and find potential issues with:
- Prepared statement caching in async mode
- Multiple concurrent connections
- Large result sets
- Connection pooling
- Error handling in async mode
- Complex query patterns
"""

import asyncio
import asyncpg
import datetime
import json
import os
import sys


async def test_multiple_anonymous_prepared_statements():
    """
    Test multiple anonymous prepared statements with same query.
    This can trigger "prepared statement already exists" errors.
    """
    print("\n=== Test: Multiple anonymous prepared statements ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Execute same query multiple times - each creates anonymous prepared statement
        for i in range(5):
            result = await conn.fetchval('SELECT $1::int', i)
            assert result == i, f"Expected {i}, got {result}"
            print(f"  Iteration {i+1}: OK")
        
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_prepared_statement_reuse():
    """
    Test reusing prepared statements across multiple executions.
    This tests pg_doorman's prepared statement cache.
    """
    print("\n=== Test: Prepared statement reuse ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Prepare statement explicitly
        stmt = await conn.prepare('SELECT $1::int + $2::int')
        
        # Execute multiple times
        for i in range(10):
            result = await stmt.fetchval(i, i * 2)
            expected = i + i * 2
            assert result == expected, f"Expected {expected}, got {result}"
        
        print(f"  Executed prepared statement 10 times: OK")
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_concurrent_connections():
    """
    Test multiple concurrent connections executing queries.
    This tests connection pooling and async mode handling.
    """
    print("\n=== Test: Concurrent connections ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    
    async def worker(worker_id, iterations):
        conn = await asyncpg.connect(db_url)
        try:
            for i in range(iterations):
                result = await conn.fetchval('SELECT $1::int', worker_id * 1000 + i)
                assert result == worker_id * 1000 + i
        finally:
            await conn.close()
    
    # Run 5 workers concurrently, each doing 10 queries
    workers = [worker(i, 10) for i in range(5)]
    await asyncio.gather(*workers)
    
    print(f"  5 workers × 10 queries = 50 total queries: OK")
    print("  ✓ Test passed")


async def test_large_result_set():
    """
    Test fetching large result sets.
    This tests buffering and data transfer in async mode.
    """
    print("\n=== Test: Large result set ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Generate 1000 rows
        rows = await conn.fetch('SELECT generate_series(1, 1000) as n, md5(random()::text) as data')
        assert len(rows) == 1000, f"Expected 1000 rows, got {len(rows)}"
        
        print(f"  Fetched {len(rows)} rows: OK")
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_transaction_with_prepared_statements():
    """
    Test prepared statements inside transactions.
    This tests transaction handling with async protocol.
    """
    print("\n=== Test: Transaction with prepared statements ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        await conn.execute('DROP TABLE IF EXISTS test_tx_ps')
        await conn.execute('CREATE TABLE test_tx_ps (id int, value text)')
        
        # Transaction with multiple prepared statements
        async with conn.transaction():
            for i in range(5):
                await conn.execute('INSERT INTO test_tx_ps VALUES ($1, $2)', i, f'value_{i}')
        
        # Verify
        count = await conn.fetchval('SELECT COUNT(*) FROM test_tx_ps')
        assert count == 5, f"Expected 5 rows, got {count}"
        
        print(f"  Inserted and verified {count} rows in transaction: OK")
        
        # Cleanup
        await conn.execute('DROP TABLE test_tx_ps')
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_error_in_prepared_statement():
    """
    Test error handling in prepared statements.
    This tests error recovery in async mode.
    """
    print("\n=== Test: Error in prepared statement ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Try to execute invalid query
        try:
            await conn.fetchval('SELECT 1 / $1::int', 0)
            assert False, "Should have raised division by zero error"
        except asyncpg.exceptions.DivisionByZeroError:
            print("  Caught division by zero error: OK")
        
        # Verify connection still works after error
        result = await conn.fetchval('SELECT 42')
        assert result == 42, f"Expected 42, got {result}"
        print("  Connection still works after error: OK")
        
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_mixed_simple_and_extended_protocol():
    """
    Test mixing simple and extended query protocol.
    This tests protocol switching.
    """
    print("\n=== Test: Mixed simple and extended protocol ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Simple query (no parameters)
        result1 = await conn.fetchval('SELECT 1')
        assert result1 == 1
        print("  Simple query: OK")
        
        # Extended query (with parameters)
        result2 = await conn.fetchval('SELECT $1::int', 2)
        assert result2 == 2
        print("  Extended query: OK")
        
        # Simple query again
        result3 = await conn.fetchval('SELECT 3')
        assert result3 == 3
        print("  Simple query again: OK")
        
        # Extended query again
        result4 = await conn.fetchval('SELECT $1::int', 4)
        assert result4 == 4
        print("  Extended query again: OK")
        
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_connection_pool():
    """
    Test connection pooling with asyncpg.
    This tests pool management and connection reuse.
    """
    print("\n=== Test: Connection pool ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    
    # Create connection pool
    pool = await asyncpg.create_pool(db_url, min_size=2, max_size=5)
    
    try:
        async def query_worker(worker_id):
            async with pool.acquire() as conn:
                result = await conn.fetchval('SELECT $1::int', worker_id)
                assert result == worker_id
                return result
        
        # Run 10 workers concurrently with pool of 5 connections
        results = await asyncio.gather(*[query_worker(i) for i in range(10)])
        assert len(results) == 10
        
        print(f"  10 workers with pool size 5: OK")
        print("  ✓ Test passed")
    finally:
        await pool.close()


async def test_cursor_iteration():
    """
    Test cursor-based iteration over large result sets.
    This tests portal handling in async mode.
    """
    print("\n=== Test: Cursor iteration ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        count = 0
        async with conn.transaction():
            # Use cursor to iterate over large result set
            async for record in conn.cursor('SELECT generate_series(1, 100) as n'):
                count += 1
                assert record['n'] == count
        
        assert count == 100, f"Expected 100 records, got {count}"
        print(f"  Iterated over {count} records with cursor: OK")
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_multiple_result_sets():
    """
    Test queries returning multiple result sets.
    This tests handling of multiple DataRow sequences.
    """
    print("\n=== Test: Multiple result sets ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Execute multiple queries in one call
        results = await conn.fetch('''
            SELECT 1 as a UNION ALL SELECT 2 UNION ALL SELECT 3
        ''')
        
        assert len(results) == 3, f"Expected 3 results, got {len(results)}"
        assert results[0]['a'] == 1
        assert results[1]['a'] == 2
        assert results[2]['a'] == 3
        
        print(f"  Fetched {len(results)} results: OK")
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_prepared_statement_with_complex_types():
    """
    Test prepared statements with complex PostgreSQL types.
    This tests type handling in async mode.
    """
    print("\n=== Test: Prepared statement with complex types ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)
    
    try:
        # Array type
        result = await conn.fetchval('SELECT $1::int[]', [1, 2, 3])
        assert result == [1, 2, 3]
        print("  Array type: OK")
        
        # JSON type - asyncpg expects string for jsonb, not dict
        # asyncpg may return string or dict depending on version
        json_data = {'key': 'value'}
        result = await conn.fetchval('SELECT $1::jsonb', json.dumps(json_data))
        # Parse result if it's a string
        if isinstance(result, str):
            result = json.loads(result)
        assert result == json_data, f"Expected {json_data}, got {result}"
        print("  JSON type: OK")
        
        # Date type
        date = datetime.date(2024, 1, 1)
        result = await conn.fetchval('SELECT $1::date', date)
        assert result == date
        print("  Date type: OK")
        
        # Timestamp type
        ts = datetime.datetime(2024, 1, 1, 12, 0, 0)
        result = await conn.fetchval('SELECT $1::timestamp', ts)
        assert result == ts
        print("  Timestamp type: OK")
        
        print("  ✓ Test passed")
    finally:
        await conn.close()


async def test_rapid_connect_disconnect():
    """
    Test rapid connection creation and destruction.
    This tests connection handling under stress.
    """
    print("\n=== Test: Rapid connect/disconnect ===")
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    
    for i in range(10):
        conn = await asyncpg.connect(db_url)
        result = await conn.fetchval('SELECT $1::int', i)
        assert result == i
        await conn.close()
    
    print(f"  10 rapid connect/disconnect cycles: OK")
    print("  ✓ Test passed")


async def main():
    """Run all edge case tests."""
    print("=" * 60)
    print("pg_doorman async protocol edge case tests")
    print("=" * 60)
    
    tests = [
        ("Multiple anonymous prepared statements", test_multiple_anonymous_prepared_statements),
        ("Prepared statement reuse", test_prepared_statement_reuse),
        ("Concurrent connections", test_concurrent_connections),
        ("Large result set", test_large_result_set),
        ("Transaction with prepared statements", test_transaction_with_prepared_statements),
        ("Error in prepared statement", test_error_in_prepared_statement),
        ("Mixed simple and extended protocol", test_mixed_simple_and_extended_protocol),
        ("Connection pool", test_connection_pool),
        ("Cursor iteration", test_cursor_iteration),
        ("Multiple result sets", test_multiple_result_sets),
        ("Prepared statement with complex types", test_prepared_statement_with_complex_types),
        ("Rapid connect/disconnect", test_rapid_connect_disconnect),
    ]
    
    passed = 0
    failed = 0
    
    for name, test_func in tests:
        try:
            await test_func()
            passed += 1
        except Exception as e:
            print(f"  ✗ Test FAILED: {e}")
            import traceback
            traceback.print_exc()
            failed += 1
    
    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed out of {len(tests)} tests")
    print("=" * 60)
    
    if failed > 0:
        sys.exit(1)


if __name__ == '__main__':
    asyncio.run(main())
