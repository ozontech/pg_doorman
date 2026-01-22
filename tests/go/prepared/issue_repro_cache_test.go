package doorman_test

import (
	"context"
	"fmt"
	"os"
	"testing"

	"github.com/jackc/pgx/v4"
	"github.com/stretchr/testify/assert"
)

func TestIssueReproPreparedCache(t *testing.T) {
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	assert.NoError(t, err)
	defer conn.Close(ctx)

	// Создаем 100 различных подготовленных операторов
	for i := 1; i <= 100; i++ {
		stmtName := fmt.Sprintf("stmt_%d", i)
		query := fmt.Sprintf("SELECT %d", i)
		_, err := conn.Prepare(ctx, stmtName, query)
		assert.NoError(t, err)
	}

	// Проверяем количество подготовленных операторов в бэкенде
	var count int
	err = conn.QueryRow(ctx, "SELECT count(*) FROM pg_prepared_statements").Scan(&count)
	assert.NoError(t, err)

	// Должно быть ровно 10, так как prepared_statements_cache_size = 10
	assert.Equal(t, 10, count, "Количество подготовленных операторов в pg_prepared_statements должно быть равно 10")
}
