package doorman_test

import (
	"database/sql"
	"os"
	"testing"

	"github.com/stretchr/testify/assert"
)

// TestApplicationName verifies that the PostgreSQL session parameter `application_name`
// is correctly set when the client connects through the pooler (pg_doorman).
//
// Why this matters
// - Observability: `application_name` is used in server logs, `pg_stat_activity`, and
//   monitoring to attribute queries to a logical client/app. If it is not propagated
//   through the pooler, operators lose traceability.
// - Contract: We lock in the behavior that the pooler preserves/sets
//   `application_name` for the backend session so that future changes don’t regress it.
//
// What the test does
// - Establishes a connection using `DATABASE_URL` that goes through the pooler.
// - Executes `SHOW application_name` on the server.
// - Asserts the value equals the expected name, confirming the pooler behavior.
func TestApplicationName(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	defer db.Close()
	var applicationName string
	assert.NoError(t, db.QueryRow(`show application_name`).Scan(&applicationName))
	assert.Equal(t, "doorman_example_user_1", applicationName)
}
