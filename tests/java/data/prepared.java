import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.Statement;

/**
 * Basic prepared statements test.
 * Creates table, inserts data using prepared statements with parameters.
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
                // Setup table
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_jdbc; CREATE TABLE test_jdbc(id serial primary key, t int)");
                }

                // Insert using prepared statements with different comments (simulating different queries)
                for (int i = 0; i < 10; i++) {
                    String sql = String.format("/*%d*/ INSERT INTO test_jdbc(t) VALUES(?)", i);
                    try (PreparedStatement pstmt = connection.prepareStatement(sql)) {
                        pstmt.setInt(1, i);
                        // Execute multiple times with same prepared statement
                        for (int j = 0; j < 10; j++) {
                            pstmt.executeUpdate();
                        }
                    }
                }

                System.out.println("prepared complete");
            }
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
