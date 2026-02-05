import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.Statement;

/**
 * PBDE test - multiple SELECT queries in single statement.
 * Tests Parse/Bind/Describe/Execute flow with multiple result sets.
 */
public class Main {
    public static void main(String[] args) throws Exception {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
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
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
