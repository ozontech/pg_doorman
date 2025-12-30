package doorman_test

import (
	"database/sql"
	"os"
	"testing"

    _ "github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func TestServerAuthMD5BAD(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL_MD5_AUTH_BAD"))
	assert.NoError(t, err)
	defer db.Close()
	assert.Error(t, db.Ping())
}

func TestServerAuthMD5OK(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL_MD5_AUTH_OK"))
	assert.NoError(t, err)
	defer db.Close()
	var dbname string
	assert.NoError(t, db.QueryRow("select current_database()").Scan(&dbname))
	assert.Equal(t, dbname, "example_db")
}
