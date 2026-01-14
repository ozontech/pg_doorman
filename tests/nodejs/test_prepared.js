/**
 * Prepared statements tests for pg_doorman with Node.js pg client.
 * Tests various corner cases with prepared statements.
 */
const { Client, Pool } = require('pg');

const DATABASE_URL = process.env.DATABASE_URL;

async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Test basic prepared statement execution
 */
async function testBasicPrepared() {
    console.log('\n=== Test: Basic prepared statement ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Create test table
        await client.query('DROP TABLE IF EXISTS nodejs_prepared_test');
        await client.query('CREATE TABLE nodejs_prepared_test (id serial PRIMARY KEY, name text, value int)');

        // Insert with parameters (uses prepared statement internally)
        for (let i = 0; i < 5; i++) {
            await client.query('INSERT INTO nodejs_prepared_test (name, value) VALUES ($1, $2)', [`name_${i}`, i * 10]);
        }

        // Select with parameters
        const result = await client.query('SELECT * FROM nodejs_prepared_test WHERE value > $1 ORDER BY id', [15]);
        if (result.rows.length !== 3) {
            throw new Error(`Expected 3 rows, got ${result.rows.length}`);
        }

        console.log('  ✓ Basic prepared statement test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test multiple executions of the same prepared statement
 */
async function testPreparedReuse() {
    console.log('\n=== Test: Prepared statement reuse ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Execute same query multiple times
        for (let i = 0; i < 10; i++) {
            const result = await client.query('SELECT $1::int as num', [i]);
            if (result.rows[0].num !== i) {
                throw new Error(`Expected ${i}, got ${result.rows[0].num}`);
            }
        }

        console.log('  ✓ Prepared statement reuse test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test prepared statements with different parameter types
 */
async function testPreparedWithTypes() {
    console.log('\n=== Test: Prepared statements with different types ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Integer
        let result = await client.query('SELECT $1::int + $2::int as sum', [10, 20]);
        if (result.rows[0].sum !== 30) {
            throw new Error(`Integer test failed: expected 30, got ${result.rows[0].sum}`);
        }
        console.log('  Integer parameters: OK');

        // Text
        result = await client.query('SELECT $1::text || $2::text as concat', ['Hello', 'World']);
        if (result.rows[0].concat !== 'HelloWorld') {
            throw new Error(`Text test failed: expected HelloWorld, got ${result.rows[0].concat}`);
        }
        console.log('  Text parameters: OK');

        // Boolean
        result = await client.query('SELECT $1::boolean AND $2::boolean as result', [true, false]);
        if (result.rows[0].result !== false) {
            throw new Error(`Boolean test failed: expected false, got ${result.rows[0].result}`);
        }
        console.log('  Boolean parameters: OK');

        // Array
        result = await client.query('SELECT $1::int[] as arr', [[1, 2, 3]]);
        if (JSON.stringify(result.rows[0].arr) !== '[1,2,3]') {
            throw new Error(`Array test failed: expected [1,2,3], got ${JSON.stringify(result.rows[0].arr)}`);
        }
        console.log('  Array parameters: OK');

        // JSON
        const jsonData = { key: 'value', num: 42 };
        result = await client.query('SELECT $1::jsonb as data', [JSON.stringify(jsonData)]);
        if (result.rows[0].data.key !== 'value' || result.rows[0].data.num !== 42) {
            throw new Error(`JSON test failed`);
        }
        console.log('  JSON parameters: OK');

        // NULL
        result = await client.query('SELECT $1::text as val', [null]);
        if (result.rows[0].val !== null) {
            throw new Error(`NULL test failed: expected null, got ${result.rows[0].val}`);
        }
        console.log('  NULL parameter: OK');

        console.log('  ✓ Prepared statements with different types test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test prepared statements in transactions
 */
async function testPreparedInTransaction() {
    console.log('\n=== Test: Prepared statements in transaction ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_tx_test');
        await client.query('CREATE TABLE nodejs_tx_test (id serial PRIMARY KEY, value int)');

        // Start transaction
        await client.query('BEGIN');

        // Insert with prepared statement
        for (let i = 0; i < 5; i++) {
            await client.query('INSERT INTO nodejs_tx_test (value) VALUES ($1)', [i]);
        }

        // Select within transaction
        let result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_tx_test');
        if (parseInt(result.rows[0].cnt) !== 5) {
            throw new Error(`Expected 5 rows in transaction, got ${result.rows[0].cnt}`);
        }

        await client.query('COMMIT');

        // Verify after commit
        result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_tx_test');
        if (parseInt(result.rows[0].cnt) !== 5) {
            throw new Error(`Expected 5 rows after commit, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Prepared statements in transaction test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test concurrent prepared statements with connection pool
 */
async function testConcurrentPrepared() {
    console.log('\n=== Test: Concurrent prepared statements ===');
    const pool = new Pool({
        connectionString: DATABASE_URL,
        max: 5
    });

    try {
        const promises = [];
        for (let i = 0; i < 20; i++) {
            promises.push(
                pool.query('SELECT $1::int * 2 as result', [i]).then(res => {
                    if (res.rows[0].result !== i * 2) {
                        throw new Error(`Concurrent test failed for i=${i}`);
                    }
                    return res.rows[0].result;
                })
            );
        }

        await Promise.all(promises);
        console.log('  ✓ Concurrent prepared statements test passed');
    } finally {
        await pool.end();
    }
}

/**
 * Test prepared statement with large number of parameters
 */
async function testManyParameters() {
    console.log('\n=== Test: Prepared statement with many parameters ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Create query with 50 parameters
        const params = [];
        const placeholders = [];
        for (let i = 1; i <= 50; i++) {
            params.push(i);
            placeholders.push(`$${i}::int`);
        }

        const query = `SELECT ${placeholders.join(' + ')} as total`;
        const result = await client.query(query, params);

        const expectedSum = (50 * 51) / 2; // Sum of 1 to 50
        if (result.rows[0].total !== expectedSum) {
            throw new Error(`Expected ${expectedSum}, got ${result.rows[0].total}`);
        }

        console.log('  ✓ Many parameters test passed');
    } finally {
        await client.end();
    }
}

// Main execution
async function main() {
    console.log('Starting prepared statements tests...');
    console.log(`DATABASE_URL: ${DATABASE_URL ? DATABASE_URL.replace(/:[^:@]+@/, ':***@') : 'not set'}`);

    try {
        await testBasicPrepared();
        await testPreparedReuse();
        await testPreparedWithTypes();
        await testPreparedInTransaction();
        await testConcurrentPrepared();
        await testManyParameters();

        console.log('\n========================================');
        console.log('All prepared statements tests passed!');
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
