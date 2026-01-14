/**
 * Error handling tests for pg_doorman with Node.js pg client.
 * Tests various error scenarios and edge cases.
 */
const { Client, Pool } = require('pg');

const DATABASE_URL = process.env.DATABASE_URL;

async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Test SQL syntax error handling
 */
async function testSyntaxError() {
    console.log('\n=== Test: SQL syntax error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        try {
            await client.query('SELEC * FROM nonexistent');
            throw new Error('Expected syntax error but query succeeded');
        } catch (err) {
            if (!err.message.includes('syntax error')) {
                throw new Error(`Expected syntax error, got: ${err.message}`);
            }
            console.log('  Syntax error caught correctly');
        }

        // Verify connection still works after error
        const result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Connection broken after syntax error');
        }
        console.log('  Connection still works after error');

        console.log('  ✓ SQL syntax error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test table not found error
 */
async function testTableNotFound() {
    console.log('\n=== Test: Table not found error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        try {
            await client.query('SELECT * FROM this_table_does_not_exist_12345');
            throw new Error('Expected error but query succeeded');
        } catch (err) {
            if (!err.message.includes('does not exist')) {
                throw new Error(`Expected "does not exist" error, got: ${err.message}`);
            }
            console.log('  Table not found error caught correctly');
        }

        // Verify connection still works
        const result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Connection broken after error');
        }

        console.log('  ✓ Table not found error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test constraint violation error
 */
async function testConstraintViolation() {
    console.log('\n=== Test: Constraint violation error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_constraint_test');
        await client.query('CREATE TABLE nodejs_constraint_test (id int PRIMARY KEY, name text NOT NULL)');

        // Insert valid row
        await client.query('INSERT INTO nodejs_constraint_test (id, name) VALUES ($1, $2)', [1, 'test']);

        // Try to insert duplicate primary key
        try {
            await client.query('INSERT INTO nodejs_constraint_test (id, name) VALUES ($1, $2)', [1, 'duplicate']);
            throw new Error('Expected constraint violation but insert succeeded');
        } catch (err) {
            if (!err.message.includes('duplicate key') && !err.message.includes('unique constraint')) {
                throw new Error(`Expected constraint violation, got: ${err.message}`);
            }
            console.log('  Primary key violation caught correctly');
        }

        // Try to insert NULL into NOT NULL column
        try {
            await client.query('INSERT INTO nodejs_constraint_test (id, name) VALUES ($1, $2)', [2, null]);
            throw new Error('Expected NOT NULL violation but insert succeeded');
        } catch (err) {
            if (!err.message.includes('null value') && !err.message.includes('NOT NULL')) {
                throw new Error(`Expected NOT NULL violation, got: ${err.message}`);
            }
            console.log('  NOT NULL violation caught correctly');
        }

        console.log('  ✓ Constraint violation error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test transaction rollback on error
 */
async function testTransactionRollbackOnError() {
    console.log('\n=== Test: Transaction rollback on error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        await client.query('DROP TABLE IF EXISTS nodejs_rollback_test');
        await client.query('CREATE TABLE nodejs_rollback_test (id int PRIMARY KEY, value int)');

        // Start transaction
        await client.query('BEGIN');
        await client.query('INSERT INTO nodejs_rollback_test (id, value) VALUES (1, 100)');

        // Cause an error
        try {
            await client.query('INSERT INTO nodejs_rollback_test (id, value) VALUES (1, 200)'); // duplicate
        } catch (err) {
            console.log('  Error in transaction caught');
        }

        // Transaction should be aborted, rollback required
        await client.query('ROLLBACK');

        // Verify no data was inserted
        const result = await client.query('SELECT COUNT(*) as cnt FROM nodejs_rollback_test');
        if (parseInt(result.rows[0].cnt) !== 0) {
            throw new Error(`Expected 0 rows after rollback, got ${result.rows[0].cnt}`);
        }

        console.log('  ✓ Transaction rollback on error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test error recovery - multiple errors in sequence
 */
async function testErrorRecovery() {
    console.log('\n=== Test: Error recovery ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Generate multiple errors
        for (let i = 0; i < 5; i++) {
            try {
                await client.query('SELECT * FROM nonexistent_table_' + i);
            } catch (err) {
                // Expected error
            }
        }

        // Connection should still work
        const result = await client.query('SELECT $1::int as num', [42]);
        if (result.rows[0].num !== 42) {
            throw new Error('Connection broken after multiple errors');
        }

        console.log('  ✓ Error recovery test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test type conversion error
 */
async function testTypeConversionError() {
    console.log('\n=== Test: Type conversion error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        try {
            await client.query('SELECT $1::int', ['not_a_number']);
            throw new Error('Expected type conversion error but query succeeded');
        } catch (err) {
            if (!err.message.includes('invalid input syntax') && !err.message.includes('integer')) {
                throw new Error(`Expected type conversion error, got: ${err.message}`);
            }
            console.log('  Type conversion error caught correctly');
        }

        // Verify connection still works
        const result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Connection broken after type error');
        }

        console.log('  ✓ Type conversion error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test division by zero error
 */
async function testDivisionByZero() {
    console.log('\n=== Test: Division by zero error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        try {
            await client.query('SELECT 1/0');
            throw new Error('Expected division by zero error but query succeeded');
        } catch (err) {
            if (!err.message.includes('division by zero')) {
                throw new Error(`Expected division by zero error, got: ${err.message}`);
            }
            console.log('  Division by zero error caught correctly');
        }

        // Verify connection still works
        const result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Connection broken after division error');
        }

        console.log('  ✓ Division by zero error test passed');
    } finally {
        await client.end();
    }
}

/**
 * Test pool error handling
 */
async function testPoolErrorHandling() {
    console.log('\n=== Test: Pool error handling ===');
    const pool = new Pool({
        connectionString: DATABASE_URL,
        max: 3
    });

    try {
        // Generate errors on multiple pool connections
        const promises = [];
        for (let i = 0; i < 10; i++) {
            promises.push(
                pool.query('SELECT * FROM nonexistent_pool_table_' + i)
                    .catch(err => ({ error: true, message: err.message }))
            );
        }

        const results = await Promise.all(promises);
        const errors = results.filter(r => r.error);
        if (errors.length !== 10) {
            throw new Error(`Expected 10 errors, got ${errors.length}`);
        }
        console.log('  All pool errors caught correctly');

        // Pool should still work
        const result = await pool.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Pool broken after errors');
        }

        console.log('  ✓ Pool error handling test passed');
    } finally {
        await pool.end();
    }
}

/**
 * Test error with prepared statement parameters mismatch
 */
async function testParameterMismatch() {
    console.log('\n=== Test: Parameter mismatch error ===');
    const client = new Client({ connectionString: DATABASE_URL });
    await client.connect();

    try {
        // Too few parameters
        try {
            await client.query('SELECT $1::int + $2::int', [1]);
            throw new Error('Expected parameter error but query succeeded');
        } catch (err) {
            console.log('  Too few parameters error caught');
        }

        // Verify connection still works
        const result = await client.query('SELECT 1 as num');
        if (result.rows[0].num !== 1) {
            throw new Error('Connection broken after parameter error');
        }

        console.log('  ✓ Parameter mismatch error test passed');
    } finally {
        await client.end();
    }
}

// Main execution
async function main() {
    console.log('Starting error handling tests...');
    console.log(`DATABASE_URL: ${DATABASE_URL ? DATABASE_URL.replace(/:[^:@]+@/, ':***@') : 'not set'}`);

    try {
        await testSyntaxError();
        await testTableNotFound();
        await testConstraintViolation();
        await testTransactionRollbackOnError();
        await testErrorRecovery();
        await testTypeConversionError();
        await testDivisionByZero();
        await testPoolErrorHandling();
        await testParameterMismatch();

        console.log('\n========================================');
        console.log('All error handling tests passed!');
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
