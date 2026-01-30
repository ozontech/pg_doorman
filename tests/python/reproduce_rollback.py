import asyncio
import asyncpg
import os
import sys

async def reproduce():
    db_url = os.getenv('DATABASE_URL_ROLLBACK', 'postgresql://example_user_rollback:test@localhost:6433/example_db')
    print(f"Connecting to {db_url}")
    conn = await asyncpg.connect(db_url)
    
    try:
        await conn.execute('DROP TABLE IF EXISTS test_savepoint')
        await conn.execute('CREATE TABLE test_savepoint (id serial PRIMARY KEY, value int)')
        
        print("Starting transaction...")
        tr = conn.transaction()
        await tr.start()
        
        try:
            await conn.execute('INSERT INTO test_savepoint (value) VALUES (1)')
            print("Creating savepoint sp...")
            await conn.execute('SAVEPOINT sp')
            
            await conn.execute('INSERT INTO test_savepoint (value) VALUES (2)')
            
            print("Executing failing query...")
            try:
                # This should fail and put transaction in aborted state
                await conn.execute('SELECT * FROM test_savepoint_unknown')
            except asyncpg.exceptions.UndefinedTableError:
                print("Caught expected UndefinedTableError")
            
            print("Rolling back to savepoint sp...")
            await conn.execute('ROLLBACK TO SAVEPOINT sp')
            
            print("Verifying data after rollback to savepoint...")
            count = await conn.fetchval('SELECT count(*) FROM test_savepoint')
            print(f"Count: {count}")
            assert count == 1, f"Expected 1, got {count}"
            
            print("Committing transaction...")
            await tr.commit()
            print("Transaction committed successfully")
            
        except Exception as e:
            print(f"Error inside transaction: {e}")
            await tr.rollback()
            raise
            
        # Final verify
        final_count = await conn.fetchval('SELECT count(*) FROM test_savepoint')
        print(f"Final count: {final_count}")
        assert final_count == 1
        
        print("✓ Reproduction test passed")
        
    finally:
        await conn.close()

if __name__ == '__main__':
    asyncio.run(reproduce())
