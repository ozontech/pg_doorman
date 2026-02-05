import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.Statement;
import java.sql.Timestamp;
import java.sql.Types;

/**
 * Advanced prepared statements test.
 * Tests different data types, statement reuse, multiple statements, and NULL values.
 */
public class Main {
    public static void main(String[] args) {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        HikariConfig config = new HikariConfig();
        config.setJdbcUrl(databaseUrl);
        config.setMaximumPoolSize(10);
        config.setMinimumIdle(1);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config)) {
            // Test 1: Prepared statements with different data types
            System.out.println("Test 1: Different data types");
            try (Connection connection = dataSource.getConnection()) {
                // Setup table
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_prepared_types; " +
                        "CREATE TABLE test_prepared_types(id serial primary key, int_val int, text_val text, " +
                        "bool_val boolean, float_val float8, ts_val timestamp)");
                }

                // Prepared insert with multiple types
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_prepared_types(int_val, text_val, bool_val, float_val, ts_val) " +
                        "VALUES(?, ?, ?, ?, ?)")) {
                    for (int i = 0; i < 10; i++) {
                        pstmt.setInt(1, i);
                        pstmt.setString(2, "text_" + i);
                        pstmt.setBoolean(3, i % 2 == 0);
                        pstmt.setDouble(4, i * 1.5);
                        pstmt.setTimestamp(5, new Timestamp(System.currentTimeMillis() + i * 86400000L));
                        pstmt.executeUpdate();
                    }
                }

                // Prepared select
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_prepared_types WHERE int_val > ?")) {
                    pstmt.setInt(1, 5);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        int count = 0;
                        while (rs.next()) count++;
                        if (count != 4) throw new RuntimeException("Expected 4 rows, got " + count);
                    }
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Reuse prepared statement across multiple executions
            System.out.println("Test 2: Reuse prepared statement");
            try (Connection connection = dataSource.getConnection()) {
                try (PreparedStatement pstmt = connection.prepareStatement("SELECT ?::int * 2 as result")) {
                    for (int i = 0; i < 100; i++) {
                        pstmt.setInt(1, i);
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            int result = rs.getInt(1);
                            if (result != i * 2) throw new RuntimeException("Expected " + (i * 2) + ", got " + result);
                        }
                    }
                }
            }
            System.out.println("Test 2 complete");

            // Test 3: Multiple prepared statements in same connection
            System.out.println("Test 3: Multiple prepared statements");
            try (Connection connection = dataSource.getConnection()) {
                try (PreparedStatement pstmt1 = connection.prepareStatement("SELECT ?::int + ?::int");
                     PreparedStatement pstmt2 = connection.prepareStatement("SELECT ?::int * ?::int")) {

                    for (int i = 0; i < 50; i++) {
                        pstmt1.setInt(1, i);
                        pstmt1.setInt(2, i + 1);
                        try (ResultSet rs = pstmt1.executeQuery()) {
                            rs.next();
                            int sum = rs.getInt(1);
                            if (sum != i + i + 1) throw new RuntimeException("Sum mismatch: expected " + (i + i + 1) + ", got " + sum);
                        }

                        pstmt2.setInt(1, i);
                        pstmt2.setInt(2, 2);
                        try (ResultSet rs = pstmt2.executeQuery()) {
                            rs.next();
                            int product = rs.getInt(1);
                            if (product != i * 2) throw new RuntimeException("Product mismatch: expected " + (i * 2) + ", got " + product);
                        }
                    }
                }
            }
            System.out.println("Test 3 complete");

            // Test 4: Prepared statement with NULL values
            System.out.println("Test 4: NULL values");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_nulls; CREATE TABLE test_nulls(id serial, val int)");
                }

                try (PreparedStatement pstmt = connection.prepareStatement("INSERT INTO test_nulls(val) VALUES(?)")) {
                    // Insert NULL
                    pstmt.setNull(1, Types.INTEGER);
                    pstmt.executeUpdate();

                    // Insert regular value
                    pstmt.setInt(1, 42);
                    pstmt.executeUpdate();

                    // Insert NULL again
                    pstmt.setNull(1, Types.INTEGER);
                    pstmt.executeUpdate();
                }

                try (Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_nulls WHERE val IS NULL")) {
                    rs.next();
                    long nullCount = rs.getLong(1);
                    if (nullCount != 2) throw new RuntimeException("Expected 2 NULL rows, got " + nullCount);
                }
            }
            System.out.println("Test 4 complete");

            System.out.println("prepared_advanced complete");
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
