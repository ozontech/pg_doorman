import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.sql.Types;

/**
 * Error handling test.
 * Tests various PostgreSQL error conditions and verifies connection remains usable after errors.
 */
public class Main {
    public static void main(String[] args) throws Exception {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        // Test 1: Syntax error handling
        System.out.println("Test 1: Syntax error");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.executeQuery("SELEC * FROM nonexistent");
                throw new RuntimeException("Expected syntax error");
            } catch (SQLException ex) {
                if (!ex.getMessage().contains("syntax error")) {
                    throw new RuntimeException("Expected syntax error, got: " + ex.getMessage());
                }
                System.out.println("Caught expected syntax error: " + ex.getSQLState());
            }

            // Connection should still be usable
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT 1")) {
                rs.next();
                if (rs.getInt(1) != 1) throw new RuntimeException("Connection broken after error");
            }
        }
        System.out.println("Test 1 complete");

        // Test 2: Table not found error
        System.out.println("Test 2: Table not found");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.executeQuery("SELECT * FROM table_that_does_not_exist_12345");
                throw new RuntimeException("Expected relation not found error");
            } catch (SQLException ex) {
                if (!"42P01".equals(ex.getSQLState())) {
                    throw new RuntimeException("Expected undefined_table error (42P01), got: " + ex.getSQLState());
                }
                System.out.println("Caught expected error: " + ex.getSQLState());
            }

            // Connection should still be usable
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT 2")) {
                rs.next();
                if (rs.getInt(1) != 2) throw new RuntimeException("Connection broken after error");
            }
        }
        System.out.println("Test 2 complete");

        // Test 3: Constraint violation (unique)
        System.out.println("Test 3: Unique constraint violation");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_unique; CREATE TABLE test_unique(id int PRIMARY KEY)");
                stmt.execute("INSERT INTO test_unique(id) VALUES(1)");
            }

            try (Statement stmt = connection.createStatement()) {
                stmt.execute("INSERT INTO test_unique(id) VALUES(1)");
                throw new RuntimeException("Expected unique violation error");
            } catch (SQLException ex) {
                if (!"23505".equals(ex.getSQLState())) {
                    throw new RuntimeException("Expected unique_violation error (23505), got: " + ex.getSQLState());
                }
                System.out.println("Caught expected error: " + ex.getSQLState());
            }

            // Connection should still be usable
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_unique")) {
                rs.next();
                long count = rs.getLong(1);
                if (count != 1) throw new RuntimeException("Expected 1 row, got " + count);
            }
        }
        System.out.println("Test 3 complete");

        // Test 4: Division by zero
        System.out.println("Test 4: Division by zero");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.executeQuery("SELECT 1/0");
                throw new RuntimeException("Expected division by zero error");
            } catch (SQLException ex) {
                if (!"22012".equals(ex.getSQLState())) {
                    throw new RuntimeException("Expected division_by_zero error (22012), got: " + ex.getSQLState());
                }
                System.out.println("Caught expected error: " + ex.getSQLState());
            }

            // Connection should still be usable
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT 3")) {
                rs.next();
                if (rs.getInt(1) != 3) throw new RuntimeException("Connection broken after error");
            }
        }
        System.out.println("Test 4 complete");

        // Test 5: Error in prepared statement
        System.out.println("Test 5: Error in prepared statement");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_prep_err; CREATE TABLE test_prep_err(id int NOT NULL)");
            }

            try (PreparedStatement pstmt = connection.prepareStatement("INSERT INTO test_prep_err(id) VALUES(?)")) {
                // First insert should succeed
                pstmt.setInt(1, 1);
                pstmt.executeUpdate();

                // NULL should fail (NOT NULL constraint)
                try {
                    pstmt.setNull(1, Types.INTEGER);
                    pstmt.executeUpdate();
                    throw new RuntimeException("Expected NOT NULL violation");
                } catch (SQLException ex) {
                    if (!"23502".equals(ex.getSQLState())) {
                        throw new RuntimeException("Expected not_null_violation error (23502), got: " + ex.getSQLState());
                    }
                    System.out.println("Caught expected error: " + ex.getSQLState());
                }

                // Prepared statement should still work
                pstmt.setInt(1, 2);
                pstmt.executeUpdate();
            }

            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_prep_err")) {
                rs.next();
                long count = rs.getLong(1);
                if (count != 2) throw new RuntimeException("Expected 2 rows, got " + count);
            }
        }
        System.out.println("Test 5 complete");

        // Test 6: Multiple errors in sequence
        System.out.println("Test 6: Multiple errors in sequence");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            for (int i = 0; i < 5; i++) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.executeQuery("SELECT * FROM nonexistent_table_" + i);
                } catch (SQLException ex) {
                    // Expected
                }

                // Verify connection still works
                try (Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT " + i)) {
                    rs.next();
                    if (rs.getInt(1) != i) throw new RuntimeException("Connection broken after error " + i);
                }
            }
        }
        System.out.println("Test 6 complete");

        System.out.println("errors complete");
    }
}
