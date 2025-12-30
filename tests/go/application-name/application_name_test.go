package doorman

import (
	"database/sql"
	"os"
	"testing"

    _ "github.com/lib/pq"
	"github.com/stretchr/testify/assert"
)

func Test_ApplicationName(t *testing.T) {
	db, err := sql.Open("postgres", os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	defer db.Close()
	var applicationName string
	assert.NoError(t, db.QueryRow(`show application_name`).Scan(&applicationName))
	assert.Equal(t, "doorman_example_user_1", applicationName)
}
