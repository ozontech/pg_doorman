package doorman_test

import (
	"database/sql"
	"os"
	"testing"

	"github.com/stretchr/testify/assert"
)

// TestAlias
// Purpose: Verify that connecting through a pool configured with an alias name
// actually routes the connection to a different, intended database.
//
// Explanation:
// - The test reads the connection string from the environment variable
//   DATABASE_URL_ALIAS. This DSN points to a pool entry identified by an alias.
// - We open a connection via that alias and run `select current_database()`.
// - If alias routing works correctly, the backend will report the real database
//   name we are connected to, which must be "example_db" for this test.
// In short: connecting to the pool using the alias should land us in another
// database (example_db), not whatever the default would be without the alias.
func TestAlias(t *testing.T) {
	// Open a connection using the alias DSN.
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL_ALIAS"))
	assert.NoError(t, err)
	defer db.Close()

	// Ask the server which database we actually connected to.
	var dbname string
	assert.NoError(t, db.QueryRow("select current_database()").Scan(&dbname))

	// Ensure the alias really routed us to the expected database.
	assert.Equal(t, "example_db", dbname)
}
