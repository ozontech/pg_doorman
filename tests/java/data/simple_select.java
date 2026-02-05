import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.ResultSet;
import java.sql.Statement;

/**
 * Simple test that connects to PostgreSQL through pg_doorman using HikariCP
 * and executes SELECT 1.
 */
public class Main {
    public static void main(String[] args) {
        // Use DATABASE_URL environment variable if set, otherwise use default
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
            try (Connection connection = dataSource.getConnection();
                 Statement statement = connection.createStatement();
                 ResultSet resultSet = statement.executeQuery("SELECT 1 AS result")) {

                if (resultSet.next()) {
                    int result = resultSet.getInt("result");
                    if (result == 1) {
                        System.out.println("simple_select complete");
                    } else {
                        System.err.println("Unexpected result: " + result);
                        System.exit(1);
                    }
                } else {
                    System.err.println("No results returned");
                    System.exit(1);
                }
            }
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
