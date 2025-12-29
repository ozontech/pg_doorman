package doorman_test

import (
	"database/sql"
	"os"
	"testing"
	"time"

	"github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func TestLibPQ(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	_, err = db.Exec("drop table if exists lib_pq_users")
	assert.NoError(t, err)
	_, err = db.Exec("create table lib_pq_users (id serial primary key, name text, dob date)")
	assert.NoError(t, err)
	tx, err := db.Begin()
	assert.NoError(t, err)
	_, err = tx.Exec("insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	_, err = tx.Exec("insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	assert.NoError(t, tx.Commit())
	_, err = db.Exec("insert into lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	tx, err = db.Begin()
	assert.NoError(t, err)
	stmt, err := tx.Prepare("select name from lib_pq_users where name = $1 limit 1")
	assert.NoError(t, err)
	var name string
	assert.NoError(t, stmt.QueryRow("dmitrivasilyev").Scan(&name))
	assert.Equal(t, "dmitrivasilyev", name)
	assert.NoError(t, tx.Commit())
	t.Log("lib pq done")
}
