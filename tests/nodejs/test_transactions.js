/**
 * Transaction tests for pg_doorman with Node.js pg client.
 * Tests various transaction scenarios and edge cases.
 */
const { Client, Pool } = require('pg');

const DATABASE_URL = process.env.DATABASE_URL;

async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Test basic transaction commit
 */
async function testBasicCommit() {
    console.log('\n=== Test: Basic transaction commit ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_tx_commit_test');
        await client.query('CREATE TABLE nodejs_tx_commit_test (id serial PRIMARY KEY, value int)');

        await client.query('BEGIN');
        await client.query('INSERT INTO nodejs_tx_commit_test (value) VALUES ($1)', [100]);
        await client.query('INSERT INTO nodejs_tx_commit_test (value) VALUES ($1)', [200]);
        await client.query('COMMIT');

        const result = await client.query('SELECT SUM(value) as total FROM nodejs_tx_commit_test');
        if (parseInt(result.rows[0].total) !== 300) {
            throw new Error(`Expected total 300, got ${result.rows[0].total}`);
        }

        console.log('  ✓ Basic transaction commit test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test basic transaction rollback
 */
async function testBasicRollback() {
    console.log('\n=== Test: Basic transaction rollback ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_tx_rollback_test');
        await client.query('CREATE TABLE nodejs_tx_rollback_test (id serial PRIMARY KEY, value int)');

        await client.query('BEGIN');
        await client.query('INSERT INTO nodejs_tx_rollback_test (value) VALUES ($1)', [100]);
        await client.query('INSERT INTO nodejs_tx_rollback_test (value) VALUES ($1)', [200]);
        await client.query('ROLLBACK');

        const result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_tx_rollback_test');
        if (parseInt(result.rows[0].cnt) !== 0) {
            throw new Error(`Expected 0 rows after rollback, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Basic transaction rollback test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test nested savepoints
 */
async function testSavepoints() {
    console.log('\n=== Test: Savepoints ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_savepoint_test');
        await client.query('CREATE TABLE nodejs_savepoint_test (id serial PRIMARY KEY, value int)');

        await client.query('BEGIN');
        await client.query('INSERT INTO nodejs_savepoint_test (value) VALUES ($1)', [1]);
        
        await client.query('SAVEPOINT sp1');
        await client.query('INSERT INTO nodejs_savepoint_test (value) VALUES ($1)', [2]);
        
        await client.query('SAVEPOINT sp2');
        await client.query('INSERT INTO nodejs_savepoint_test (value) VALUES ($1)', [3]);
        
        // Rollback to sp2 - should remove value 3
        await client.query('ROLLBACK TO SAVEPOINT sp2');
        
        // Insert new value
        await client.query('INSERT INTO nodejs_savepoint_test (value) VALUES ($1)', [4]);
        
        // Rollback to sp1 - should remove values 2 and 4
        await client.query('ROLLBACK TO SAVEPOINT sp1');
        
        await client.query('COMMIT');

        const result = await client.query('SELECT value FROM nodejs_savepoint_test ORDER BY id');
        if (result.rows.length !== 1 || result.rows[0].value !== 1) {
            throw new Error(`Expected only value 1, got ${JSON.stringify(result.rows)}`);
        }

        console.log('  ✓ Savepoints test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test transaction isolation - read committed
 */
async function testReadCommittedIsolation() {
    console.log('\n=== Test: Read committed isolation ===');
    const client1 = new Client({ connectionString: DATABASE_URL });
    const client2 = new Client({ connectionString: DATABASE_URL });
    await client1.connect();
    await client2.connect();

    try {
        await client1.query('DROP TABLE IF EXISTS nodejs_isolation_test');
        await client1.query('CREATE TABLE nodejs_isolation_test (id int PRIMARY KEY, value int)');
        await client1.query('INSERT INTO nodejs_isolation_test (id, value) VALUES (1, 100)');

        // Client 1 starts transaction and updates
        await client1.query('BEGIN');
        await client1.query('UPDATE nodejs_isolation_test SET value = 200 WHERE id = 1');

        // Client 2 should see old value (100) because client1 hasn't committed
        const result1 = await client2.query('SELECT value FROM nodejs_isolation_test WHERE id = 1');
        if (result1.rows[0].value !== 100) {
            throw new Error(`Client2 should see 100 before commit, got ${result1.rows[0].value}`);
        }
        console.log('  Before commit: client2 sees old value (100)');

        // Client 1 commits
        await client1.query('COMMIT');

        // Now client 2 should see new value (200)
        const result2 = await client2.query('SELECT value FROM nodejs_isolation_test WHERE id = 1');
        if (result2.rows[0].value !== 200) {
            throw new Error(`Client2 should see 200 after commit, got ${result2.rows[0].value}`);
        }
        console.log('  After commit: client2 sees new value (200)');

        console.log('  ✓ Read committed isolation test passed');
    } finally {
        await client1.end();
        await client2.end();
    }
}

/**
 * Test multiple sequential transactions on same connection
 */
async function testSequentialTransactions() {
    console.log('\n=== Test: Sequential transactions ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_seq_tx_test');
        await client.query('CREATE TABLE nodejs_seq_tx_test (id serial PRIMARY KEY, value int)');

        // Multiple transactions on same connection
        for (let i = 0; i < 5; i++) {
            await client.query('BEGIN');
            await client.query('INSERT INTO nodejs_seq_tx_test (value) VALUES ($1)', [i * 10]);
            if (i % 2 === 0) {
                await client.query('COMMIT');
            } else {
                await client.query('ROLLBACK');
            }
        }

        // Should have 3 rows (i=0, 2, 4 committed)
        const result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_seq_tx_test');
        if (parseInt(result.rows[0].cnt) !== 3) {
            throw new Error(`Expected 3 rows, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Sequential transactions test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test transaction with SELECT FOR UPDATE
 */
async function testSelectForUpdate() {
    console.log('\n=== Test: SELECT FOR UPDATE ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_for_update_test');
        await client.query('CREATE TABLE nodejs_for_update_test (id int PRIMARY KEY, value int)');
        await client.query('INSERT INTO nodejs_for_update_test (id, value) VALUES (1, 100)');

        await client.query('BEGIN');
        
        // Lock the row
        const result = await client.query('SELECT * FROM nodejs_for_update_test WHERE id = 1 FOR UPDATE');
        if (result.rows[0].value !== 100) {
            throw new Error(`Expected value 100, got ${result.rows[0].value}`);
        }

        // Update the locked row
        await client.query('UPDATE nodejs_for_update_test SET value = 200 WHERE id = 1');
        
        await client.query('COMMIT');

        // Verify update
        const finalResult = await client.query('SELECT value FROM nodejs_for_update_test WHERE id = 1');
        if (finalResult.rows[0].value !== 200) {
            throw new Error(`Expected value 200 after update, got ${finalResult.rows[0].value}`);
        }

        console.log('  ✓ SELECT FOR UPDATE test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test transaction with prepared statements
 */
async function testTransactionWithPrepared() {
    console.log('\n=== Test: Transaction with prepared statements ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_tx_prepared_test');
        await client.query('CREATE TABLE nodejs_tx_prepared_test (id serial PRIMARY KEY, name text, value int)');

        await client.query('BEGIN');

        // Use prepared statements within transaction
        for (let i = 0; i < 10; i++) {
            await client.query('INSERT INTO nodejs_tx_prepared_test (name, value) VALUES ($1, $2)', [`item_${i}`, i * 100]);
        }

        // Query with prepared statement
        const result = await client.query('SELECT COUNT(*) as cnt, SUM(value) as total FROM nodejs_tx_prepared_test WHERE value >= $1', [500]);
        
        await client.query('COMMIT');

        if (parseInt(result.rows[0].cnt) !== 5) {
            throw new Error(`Expected 5 rows with value >= 500, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Transaction with prepared statements test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test pool transactions
 */
async function testPoolTransactions() {
    console.log('\n=== Test: Pool transactions ===');
    const pool = new Pool({
        connectionString: DATABASE_URL,
        max: 5
    });

    try {
        // Get a client from pool for transaction
        const client = await pool.connect();

        try {
            await client.query('DROP TABLE IF EXISTS nodejs_pool_tx_test');
            await client.query('CREATE TABLE nodejs_pool_tx_test (id serial PRIMARY KEY, value int)');

            await client.query('BEGIN');
            await client.query('INSERT INTO nodejs_pool_tx_test (value) VALUES ($1)', [100]);
            await client.query('INSERT INTO nodejs_pool_tx_test (value) VALUES ($1)', [200]);
            await client.query('COMMIT');
        } finally {
            client.release();
        }

        // Verify with another connection from pool
        const result = await pool.query('SELECT SUM(value) as total FROM nodejs_pool_tx_test');
        if (parseInt(result.rows[0].total) !== 300) {
            throw new Error(`Expected total 300, got ${result.rows[0].total}`);
        }

        console.log('  ✓ Pool transactions test passed');
    } finally {
        await pool.end();
    }
}

/**
 * Test concurrent transactions
 */
async function testConcurrentTransactions() {
    console.log('\n=== Test: Concurrent transactions ===');
    const pool = new Pool({
        connectionString: DATABASE_URL,
        max: 10
    });

    try {
        await pool.query('DROP TABLE IF EXISTS nodejs_concurrent_tx_test');
        await pool.query('CREATE TABLE nodejs_concurrent_tx_test (id serial PRIMARY KEY, worker_id int, value int)');

        const promises = [];
        for (let workerId = 0; workerId < 5; workerId++) {
            promises.push((async () => {
                const client = await pool.connect();
                try {
                    await client.query('BEGIN');
                    for (let i = 0; i < 3; i++) {
                        await client.query(
                            'INSERT INTO nodejs_concurrent_tx_test (worker_id, value) VALUES ($1, $2)',
                            [workerId, workerId * 100 + i]
                        );
                    }
                    await client.query('COMMIT');
                } finally {
                    client.release();
                }
            })());
        }

        await Promise.all(promises);

        // Verify all inserts
        const result = await pool.query('SELECT COUNT(*) as cnt FROM nodejs_concurrent_tx_test');
        if (parseInt(result.rows[0].cnt) !== 15) {
            throw new Error(`Expected 15 rows, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Concurrent transactions test passed');
    } finally {
        await pool.end();
    }
}

/**
 * Test transaction timeout behavior
 */
async function testIdleInTransaction() {
    console.log('\n=== Test: Idle in transaction ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_idle_tx_test');
        await client.query('CREATE TABLE nodejs_idle_tx_test (id serial PRIMARY KEY, value int)');

        await client.query('BEGIN');
        await client.query('INSERT INTO nodejs_idle_tx_test (value) VALUES ($1)', [100]);
        
        // Simulate idle time in transaction
        await sleep(100);
        
        // Should still be able to continue
        await client.query('INSERT INTO nodejs_idle_tx_test (value) VALUES ($1)', [200]);
        await client.query('COMMIT');

        const result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_idle_tx_test');
        if (parseInt(result.rows[0].cnt) !== 2) {
            throw new Error(`Expected 2 rows, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Idle in transaction test passed');
    } finally {
        await client.end();
    }
}

// Main execution
async function main() {
    console.log('Starting transaction tests...');
    console.log(`DATABASE_URL: ${DATABASE_URL ? DATABASE_URL.replace(/:[^:@]+@/, ':***@') : 'not set'}`);

    try {
        await testBasicCommit();
        await testBasicRollback();
        await testSavepoints();
        await testReadCommittedIsolation();
        await testSequentialTransactions();
        await testSelectForUpdate();
        await testTransactionWithPrepared();
        await testPoolTransactions();
        await testConcurrentTransactions();
        await testIdleInTransaction();

        console.log('\n========================================');
        console.log('All transaction tests passed!');
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
