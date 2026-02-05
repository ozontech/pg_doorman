import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.ResultSet;
import java.sql.Statement;

/**
 * Batch operations test.
 * Tests JDBC batch execution with multiple INSERT and SELECT statements.
 */
public class Main {
    public static void main(String[] args) throws Exception {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            // Setup table
            try (Statement stmt = connection.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_jdbc_batch; CREATE TABLE test_jdbc_batch(id serial primary key, t int)");
            }

            // Batch 1: multiple inserts using addBatch
            try (Statement stmt = connection.createStatement()) {
                stmt.addBatch("INSERT INTO test_jdbc_batch(t) VALUES (1)");
                stmt.addBatch("INSERT INTO test_jdbc_batch(t) VALUES (2)");
                stmt.executeBatch();
            }

            // Verify batch 1
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_jdbc_batch")) {
                rs.next();
                int count = rs.getInt(1);
                if (count != 2) {
                    throw new RuntimeException("batch 1: Expected 2 rows, got " + count);
                }
            }
            System.out.println("batch 1 complete");

            // Batch 2: more inserts
            try (Statement stmt = connection.createStatement()) {
                stmt.addBatch("INSERT INTO test_jdbc_batch(t) VALUES (3)");
                stmt.addBatch("INSERT INTO test_jdbc_batch(t) VALUES (4)");
                stmt.executeBatch();
            }

            // Verify batch 2
            try (Statement stmt = connection.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_jdbc_batch")) {
                rs.next();
                int count = rs.getInt(1);
                if (count != 4) {
                    throw new RuntimeException("batch 2: Expected 4 rows, got " + count);
                }
            }
            System.out.println("batch 2 complete");

        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
