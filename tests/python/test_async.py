import asyncio
import asyncpg
import datetime
import os

async def main():
    # Use DATABASE_URL environment variable if set, otherwise use default
    db_url = os.getenv('DATABASE_URL', 'postgresql://example_user_1:test@localhost:6433/example_db')
    conn = await asyncpg.connect(db_url)

    print("bad tx")
    tr = conn.transaction()
    try:
        await tr.start()
        await conn.execute('raise error')
    except Exception as e:
        await tr.rollback()
    else:
        tr.commit()

    print("good tx")
    tr = conn.transaction()
    try:
        await tr.start()
        await conn.execute('select 1')
    except Exception as e:
        await tr.rollback()
    else:
        await tr.commit()

    print("create table")
    await conn.execute("drop table if exists users_python;")
    await conn.execute('''
        create table users_python(
            id serial primary key,
            name text,
            dob date
        )
    ''')

    print("copy")
    result = await conn.copy_records_to_table(
        'users_python', records = [
            (1, 'Alexign', datetime.date(1983, 12, 12)),
            (2, 'Gsmolkin', datetime.date(1983, 12, 12)),
        ])
    print(result)

    print("insert")
    await conn.execute("alter sequence users_python_id_seq restart 100")
    await conn.execute('''
        insert into users_python(name, dob) values($1, $2)
    ''', 'Dima', datetime.date(1983, 12, 12))

    print("fetch")
    row = await conn.fetchrow(
        'select * from users_python where name = $1', 'Dima')

    print("rows")
    rows = await conn.fetch("select * from users_python")
    data = [dict(row) for row in rows]

    # Close the connection.
    await conn.close()

asyncio.run(main())
