package doorman_test

import (
	"database/sql"
	"os"
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestHbaTrust(t *testing.T) {
	db, errOpen := sql.Open("postgres", os.Getenv("DATABASE_URL_TRUST"))
	assert.NoError(t, errOpen)
	defer db.Close()
	_, err := db.Exec("select 1")
	assert.NoError(t, err)
}

func TestHbaDeny(t *testing.T) {
	db, errOpen := sql.Open("postgres", os.Getenv("DATABASE_URL_NOTRUST"))
	assert.NoError(t, errOpen)
	defer db.Close()
	_, err := db.Exec("select 1")
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "Connection from IP address 127.0.0.1 to example_user_nopassword@example_db (TLS: false)")
}
