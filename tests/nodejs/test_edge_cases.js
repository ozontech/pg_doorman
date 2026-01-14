/**
 * Edge case tests for pg_doorman with Node.js pg client.
 * Tests async patterns, concurrent operations, and corner cases.
 */
const { Client, Pool } = require('pg');

const DATABASE_URL = process.env.DATABASE_URL;

async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Test rapid connect/disconnect cycles
 */
async function testRapidConnectDisconnect() {
    console.log('\n=== Test: Rapid connect/disconnect ===');

    for (let i = 0; i < 10; i++) {
        const client = new Client({ connectionString: DATABASE_URL });
        await client.connect();
        const result = await client.query('SELECT $1::int as num', [i]);
        if (result.rows[0].num !== i) {
            throw new Error(`Expected ${i}, got ${result.rows[0].num}`);
        }
        await client.end();
    }

    console.log('  ✓ Rapid connect/disconnect test passed');
}

/**
 * Test multiple concurrent connections
 */
async function testConcurrentConnections() {
    console.log('\n=== Test: Concurrent connections ===');

    const promises = [];
    for (let i = 0; i < 10; i++) {
        promises.push((async () => {
            const client = new Client({ connectionString: DATABASE_URL });
            await client.connect();
            try {
                const result = await client.query('SELECT $1::int * 2 as result', [i]);
                if (result.rows[0].result !== i * 2) {
                    throw new Error(`Worker ${i}: expected ${i * 2}, got ${result.rows[0].result}`);
                }
                return i;
            } finally {
                await client.end();
            }
        })());
    }

    const results = await Promise.all(promises);
    if (results.length !== 10) {
        throw new Error(`Expected 10 results, got ${results.length}`);
    }

    console.log('  ✓ Concurrent connections test passed');
}

/**
 * Test large result set handling
 */
async function testLargeResultSet() {
    console.log('\n=== Test: Large result set ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Generate large result set
        const result = await client.query('SELECT generate_series(1, 1000) as num');
        if (result.rows.length !== 1000) {
            throw new Error(`Expected 1000 rows, got ${result.rows.length}`);
        }

        // Verify first and last values
        if (result.rows[0].num !== 1 || result.rows[999].num !== 1000) {
            throw new Error('Large result set values incorrect');
        }

        console.log('  ✓ Large result set test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test large data insertion
 */
async function testLargeDataInsertion() {
    console.log('\n=== Test: Large data insertion ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_large_data_test');
        await client.query('CREATE TABLE nodejs_large_data_test (id serial PRIMARY KEY, data text)');

        // Insert large text data
        const largeText = 'x'.repeat(10000);
        await client.query('INSERT INTO nodejs_large_data_test (data) VALUES ($1)', [largeText]);

        const result = await client.query('SELECT LENGTH(data) as len FROM nodejs_large_data_test');
        if (result.rows[0].len !== 10000) {
            throw new Error(`Expected length 10000, got ${result.rows[0].len}`);
        }

        console.log('  ✓ Large data insertion test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test empty result handling
 */
async function testEmptyResult() {
    console.log('\n=== Test: Empty result handling ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_empty_test');
        await client.query('CREATE TABLE nodejs_empty_test (id int)');

        // Query empty table
        const result = await client.query('SELECT * FROM nodejs_empty_test');
        if (result.rows.length !== 0) {
            throw new Error(`Expected 0 rows, got ${result.rows.length}`);
        }

        // Query with WHERE that matches nothing
        await client.query('INSERT INTO nodejs_empty_test (id) VALUES (1)');
        const result2 = await client.query('SELECT * FROM nodejs_empty_test WHERE id = 999');
        if (result2.rows.length !== 0) {
            throw new Error(`Expected 0 rows for non-matching WHERE, got ${result2.rows.length}`);
        }

        console.log('  ✓ Empty result handling test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test NULL value handling
 */
async function testNullHandling() {
    console.log('\n=== Test: NULL value handling ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_null_test');
        await client.query('CREATE TABLE nodejs_null_test (id int, name text, value int)');

        // Insert with NULLs
        await client.query('INSERT INTO nodejs_null_test (id, name, value) VALUES ($1, $2, $3)', [1, null, null]);
        await client.query('INSERT INTO nodejs_null_test (id, name, value) VALUES ($1, $2, $3)', [2, 'test', null]);
        await client.query('INSERT INTO nodejs_null_test (id, name, value) VALUES ($1, $2, $3)', [3, null, 100]);

        const result = await client.query('SELECT * FROM nodejs_null_test ORDER BY id');
        
        if (result.rows[0].name !== null || result.rows[0].value !== null) {
            throw new Error('Row 1 NULL values incorrect');
        }
        if (result.rows[1].name !== 'test' || result.rows[1].value !== null) {
            throw new Error('Row 2 values incorrect');
        }
        if (result.rows[2].name !== null || result.rows[2].value !== 100) {
            throw new Error('Row 3 values incorrect');
        }

        console.log('  ✓ NULL value handling test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test special characters in data
 */
async function testSpecialCharacters() {
    console.log('\n=== Test: Special characters ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_special_chars_test');
        await client.query('CREATE TABLE nodejs_special_chars_test (id serial PRIMARY KEY, data text)');

        const testStrings = [
            "Hello'World",
            'Quote"Test',
            'Back\\slash',
            'New\nLine',
            'Tab\tChar',
            'Unicode: 你好世界 🎉',
            'SQL injection: \'; DROP TABLE users; --',
            '<script>alert("xss")</script>',
            'Carriage\rReturn',
        ];

        for (const str of testStrings) {
            await client.query('INSERT INTO nodejs_special_chars_test (data) VALUES ($1)', [str]);
        }

        const result = await client.query('SELECT data FROM nodejs_special_chars_test ORDER BY id');
        
        // Verify we got all rows back (null byte might be truncated)
        if (result.rows.length !== testStrings.length) {
            throw new Error(`Expected ${testStrings.length} rows, got ${result.rows.length}`);
        }

        // Verify some specific values
        if (result.rows[0].data !== "Hello'World") {
            throw new Error('Single quote handling failed');
        }
        if (!result.rows[5].data.includes('你好世界')) {
            throw new Error('Unicode handling failed');
        }

        console.log('  ✓ Special characters test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test pool exhaustion and recovery
 */
async function testPoolExhaustion() {
    console.log('\n=== Test: Pool exhaustion and recovery ===');
    const pool = new Pool({
        connectionString: DATABASE_URL,
        max: 2,
        connectionTimeoutMillis: 5000
    });

    try {
        // Get all connections from pool
        const client1 = await pool.connect();
        const client2 = await pool.connect();

        // Start queries on both
        const promise1 = client1.query('SELECT pg_sleep(0.1), 1 as num');
        const promise2 = client2.query('SELECT pg_sleep(0.1), 2 as num');

        // Wait for queries to complete
        const [result1, result2] = await Promise.all([promise1, promise2]);

        // Release connections
        client1.release();
        client2.release();

        // Pool should work again
        const result = await pool.query('SELECT 3 as num');
        if (result.rows[0].num !== 3) {
            throw new Error('Pool recovery failed');
        }

        console.log('  ✓ Pool exhaustion and recovery test passed');
    } finally {
        await pool.end();
    }
}

/**
 * Test COPY TO STDOUT command
 */
async function testCopyCommand() {
    console.log('\n=== Test: COPY command ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_copy_test');
        await client.query('CREATE TABLE nodejs_copy_test (id int, name text)');

        // Insert test data
        await client.query('INSERT INTO nodejs_copy_test VALUES (1, $1), (2, $2)', ['Alice', 'Bob']);

        // Test COPY TO STDOUT (returns data as text)
        const result = await client.query('COPY nodejs_copy_test TO STDOUT WITH (FORMAT csv)');
        
        // Verify data was inserted
        const countResult = await client.query('SELECT COUNT(*) as cnt FROM nodejs_copy_test');
        if (parseInt(countResult.rows[0].cnt) !== 2) {
            throw new Error(`Expected 2 rows, got ${countResult.rows[0].cnt}`);
        }

        console.log('  ✓ COPY command test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test multiple statements in single query (when supported)
 */
async function testMultipleStatements() {
    console.log('\n=== Test: Multiple statements ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_multi_stmt_test');
        await client.query('CREATE TABLE nodejs_multi_stmt_test (id serial PRIMARY KEY, value int)');

        // Execute multiple inserts
        await client.query('INSERT INTO nodejs_multi_stmt_test (value) VALUES (1)');
        await client.query('INSERT INTO nodejs_multi_stmt_test (value) VALUES (2)');
        await client.query('INSERT INTO nodejs_multi_stmt_test (value) VALUES (3)');

        const result = await client.query('SELECT SUM(value) as total FROM nodejs_multi_stmt_test');
        if (parseInt(result.rows[0].total) !== 6) {
            throw new Error(`Expected total 6, got ${result.rows[0].total}`);
        }

        console.log('  ✓ Multiple statements test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test connection reuse after idle
 */
async function testConnectionAfterIdle() {
    console.log('\n=== Test: Connection after idle ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Initial query
        let result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Initial query failed');
        }

        // Wait idle
        await sleep(500);

        // Query after idle
        result = await client.query('SELECT 2 as num');
        if (result.rows[0].num !== 2) {
            throw new Error('Query after idle failed');
        }

        console.log('  ✓ Connection after idle test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test binary data handling
 */
async function testBinaryData() {
    console.log('\n=== Test: Binary data handling ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_binary_test');
        await client.query('CREATE TABLE nodejs_binary_test (id serial PRIMARY KEY, data bytea)');

        // Create binary data
        const binaryData = Buffer.from([0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD]);
        await client.query('INSERT INTO nodejs_binary_test (data) VALUES ($1)', [binaryData]);

        const result = await client.query('SELECT data FROM nodejs_binary_test');
        const retrievedData = result.rows[0].data;

        if (!Buffer.isBuffer(retrievedData)) {
            throw new Error('Retrieved data is not a Buffer');
        }
        if (retrievedData.length !== binaryData.length) {
            throw new Error(`Binary data length mismatch: expected ${binaryData.length}, got ${retrievedData.length}`);
        }
        if (!retrievedData.equals(binaryData)) {
            throw new Error('Binary data content mismatch');
        }

        console.log('  ✓ Binary data handling test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test date/time handling
 */
async function testDateTimeHandling() {
    console.log('\n=== Test: Date/time handling ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_datetime_test');
        await client.query(`CREATE TABLE nodejs_datetime_test (
            id serial PRIMARY KEY,
            date_col date,
            time_col time,
            timestamp_col timestamp,
            timestamptz_col timestamptz
        )`);

        const testDate = new Date('2024-06-15T14:30:00Z');
        await client.query(
            'INSERT INTO nodejs_datetime_test (date_col, time_col, timestamp_col, timestamptz_col) VALUES ($1, $2, $3, $4)',
            [testDate, '14:30:00', testDate, testDate]
        );

        const result = await client.query('SELECT * FROM nodejs_datetime_test');
        
        // Verify date was stored and retrieved
        if (!result.rows[0].date_col) {
            throw new Error('Date column is null');
        }
        if (!result.rows[0].timestamp_col) {
            throw new Error('Timestamp column is null');
        }

        console.log('  ✓ Date/time handling test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test numeric precision
 */
async function testNumericPrecision() {
    console.log('\n=== Test: Numeric precision ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Test bigint
        const bigintResult = await client.query('SELECT $1::bigint as num', ['9223372036854775807']);
        if (bigintResult.rows[0].num !== '9223372036854775807') {
            throw new Error('Bigint precision lost');
        }
        console.log('  Bigint precision: OK');

        // Test numeric/decimal
        const numericResult = await client.query('SELECT $1::numeric as num', ['123456789.123456789']);
        if (numericResult.rows[0].num !== '123456789.123456789') {
            throw new Error('Numeric precision lost');
        }
        console.log('  Numeric precision: OK');

        // Test float
        const floatResult = await client.query('SELECT $1::float8 as num', [3.14159265358979]);
        if (Math.abs(floatResult.rows[0].num - 3.14159265358979) > 0.0000001) {
            throw new Error('Float precision issue');
        }
        console.log('  Float precision: OK');

        console.log('  ✓ Numeric precision test passed');
    } finally {
        await client.end();
    }
}

// Main execution
async function main() {
    console.log('Starting edge case tests...');
    console.log(`DATABASE_URL: ${DATABASE_URL ? DATABASE_URL.replace(/:[^:@]+@/, ':***@') : 'not set'}`);

    try {
        await testRapidConnectDisconnect();
        await testConcurrentConnections();
        await testLargeResultSet();
        await testLargeDataInsertion();
        await testEmptyResult();
        await testNullHandling();
        await testSpecialCharacters();
        await testPoolExhaustion();
        await testCopyCommand();
        await testMultipleStatements();
        await testConnectionAfterIdle();
        await testBinaryData();
        await testDateTimeHandling();
        await testNumericPrecision();

        console.log('\n========================================');
        console.log('All edge case tests passed!');
        console.log('========================================');
        process.exit(0);
    } catch (err) {
        console.error('\n========================================');
        console.error('TEST FAILED:', err.message);
        console.error('========================================');
        console.error(err.stack);
        process.exit(1);
    }
}

main();
