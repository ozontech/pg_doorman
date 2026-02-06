import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.Statement;

/**
 * PBDE test - multiple SELECT queries in single statement.
 * Tests Parse/Bind/Describe/Execute flow with multiple result sets.
 */
public class Main {
    public static void main(String[] args) {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        HikariConfig config = new HikariConfig();
        config.setJdbcUrl(databaseUrl);
        config.setMaximumPoolSize(5);
        config.setMinimumIdle(1);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config)) {
            try (Connection connection = dataSource.getConnection()) {
                // Execute multiple SELECT statements
                try (Statement stmt = connection.createStatement()) {
                    // First execution
                    boolean hasResults = stmt.execute("SELECT 1 AS test; SELECT 2 AS test;");
                    while (hasResults || stmt.getUpdateCount() != -1) {
                        if (hasResults) {
                            stmt.getResultSet().close();
                        }
                        hasResults = stmt.getMoreResults();
                    }
                }

                // Second execution (same query)
                try (Statement stmt = connection.createStatement()) {
                    boolean hasResults = stmt.execute("SELECT 1 AS test; SELECT 2 AS test;");
                    while (hasResults || stmt.getUpdateCount() != -1) {
                        if (hasResults) {
                            stmt.getResultSet().close();
                        }
                        hasResults = stmt.getMoreResults();
                    }
                }

                System.out.println("pbde complete");
            }
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
