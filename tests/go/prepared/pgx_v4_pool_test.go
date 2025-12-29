package doorman_test

import (
	"context"
	"os"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jackc/pgx/v4/pgxpool"
	"github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func TestPgxV4Prepared(t *testing.T) {
	ctx := context.Background()
	db, err := pgxpool.Connect(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "drop table if exists prepared_pgxv4_users")
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "create table prepared_pgxv4_users (id serial primary key, name text, dob date)")
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "insert into prepared_pgxv4_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	_, err = db.Exec(ctx, "insert into prepared_pgxv4_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	concurrency := make(chan struct{}, 100)
	var count uint32
	var wg sync.WaitGroup
	for {
		concurrency <- struct{}{}
		if atomic.LoadUint32(&count) >= 20000 {
			break
		}
		wg.Add(1)
		go func() {
			defer func() {
				<-concurrency
				wg.Done()
			}()
			tx, err := db.Begin(ctx)
			assert.NoError(t, err)
			var name string
			if err := tx.QueryRow(ctx, "select name from prepared_pgxv4_users limit 1").Scan(&name); err != nil { // unamed prepred
				assert.NoError(t, err)
			}
			atomic.AddUint32(&count, 1)
			if atomic.LoadUint32(&count)%1000 == 0 {
				var preparedCount, backendPid, memory int
				if err := tx.QueryRow(ctx, "select count(*), pg_backend_pid() from pg_prepared_statements").Scan(
					&preparedCount, &backendPid); err != nil {
					assert.NoError(t, err)
				}
				assert.True(t, preparedCount < 10)
				if err := tx.QueryRow(ctx, "select sum(used_bytes) from pg_backend_memory_contexts").Scan(&memory); err != nil {
					assert.NoError(t, err)
				}
				assert.NoError(t, err)
				t.Logf("backend: %d prepared count: %d memory: %d\n", backendPid, preparedCount, memory)
			}
			assert.NoError(t, tx.Commit(ctx))
		}()
	}
	wg.Wait()
}
