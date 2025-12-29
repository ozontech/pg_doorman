package doorman_test

import (
	"database/sql"
	"os"
	"testing"
	"time"

	"github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func TestLibPQOnePrepared(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	db.SetMaxOpenConns(1)
	_, err = db.Exec("drop table if exists prepared_lib_pq_users")
	assert.NoError(t, err)
	_, err = db.Exec("create table prepared_lib_pq_users (id serial primary key, name text, dob date)")
	assert.NoError(t, err)
	_, err = db.Exec("insert into prepared_lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	_, err = db.Exec("insert into prepared_lib_pq_users (name, dob) values (unnest($1::text[]), unnest($2::date[]))",
		pq.Array([]string{"dmitrivasilyev", "alexign"}),
		pq.Array([]time.Time{
			time.Date(1983, 12, 12, 0, 0, 0, 0, time.UTC),
			time.Date(1981, 7, 19, 0, 0, 0, 0, time.UTC)}))
	assert.NoError(t, err)
	tx, err := db.Begin()
	assert.NoError(t, err)
	name := "dmitrivasilyev"
	stmt, err := tx.Prepare("select name from prepared_lib_pq_users where name = $1 limit 1")                               // named prepare
	if err := tx.QueryRow("select name from prepared_lib_pq_users where name = $1 limit 1", name).Scan(&name); err != nil { // unamed prepred
		assert.NoError(t, err)
	}
	assert.NoError(t, err)
	assert.NoError(t, stmt.Close())
	assert.NoError(t, tx.Commit())
	t.Log("done!")
}
