import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;

/**
 * Describe flow with cached prepared statements test.
 * Tests that pg_doorman correctly handles Describe when Parse is cached/skipped.
 */
public class Main {
    private static String databaseUrl;

    public static void main(String[] args) throws Exception {
        databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        System.out.println("Test: Describe flow with cached prepared statement");

        // Test 1: Basic Describe flow - first Prepare sends Parse, second reuses cache
        System.out.println("Test 1: Basic cached Describe flow");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            try (Statement stmt = connection.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_describe_cached; " +
                    "CREATE TABLE test_describe_cached(id serial primary key, value text)");
            }

            // First Prepare - Parse is sent to server
            try (PreparedStatement pstmt1 = connection.prepareStatement(
                    "SELECT * FROM test_describe_cached WHERE id = ?")) {
                pstmt1.setInt(1, 1);
                try (ResultSet rs = pstmt1.executeQuery()) {
                    // No rows expected, just verify it works
                }
            }

            // Second Prepare with SAME query - Parse should be cached/skipped
            try (PreparedStatement pstmt2 = connection.prepareStatement(
                    "SELECT * FROM test_describe_cached WHERE id = ?")) {
                pstmt2.setInt(1, 1);
                try (ResultSet rs = pstmt2.executeQuery()) {
                    // No rows expected
                }
            }
        }
        System.out.println("Test 1 complete");

        // Test 2: Multiple cached Prepare calls in sequence
        System.out.println("Test 2: Multiple cached Prepare calls");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            String query = "SELECT * FROM test_describe_cached WHERE id > ? ORDER BY id";

            for (int i = 0; i < 10; i++) {
                try (PreparedStatement pstmt = connection.prepareStatement(query)) {
                    pstmt.setInt(1, 0);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        // Just verify no protocol errors
                    }
                }
            }
        }
        System.out.println("Test 2 complete");

        // Test 3: Interleaved Prepare of different queries (cache should work per-query)
        System.out.println("Test 3: Interleaved different queries");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            // Insert some test data
            try (Statement stmt = connection.createStatement()) {
                stmt.execute("INSERT INTO test_describe_cached(value) VALUES('test1'), ('test2'), ('test3')");
            }

            String queryA = "SELECT id FROM test_describe_cached WHERE id = ?";
            String queryB = "SELECT value FROM test_describe_cached WHERE id = ?";
            String queryC = "SELECT id, value FROM test_describe_cached WHERE id = ?";

            for (int round = 0; round < 5; round++) {
                // Query A
                try (PreparedStatement pstmt = connection.prepareStatement(queryA)) {
                    pstmt.setInt(1, 1);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("Round " + round + " A: No row");
                        if (rs.getInt(1) != 1) throw new RuntimeException("Round " + round + " A: Wrong id");
                    }
                }

                // Query B
                try (PreparedStatement pstmt = connection.prepareStatement(queryB)) {
                    pstmt.setInt(1, 2);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("Round " + round + " B: No row");
                        if (!"test2".equals(rs.getString(1))) throw new RuntimeException("Round " + round + " B: Wrong value");
                    }
                }

                // Query C
                try (PreparedStatement pstmt = connection.prepareStatement(queryC)) {
                    pstmt.setInt(1, 3);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("Round " + round + " C: No row");
                        if (rs.getInt(1) != 3) throw new RuntimeException("Round " + round + " C: Wrong id");
                        if (!"test3".equals(rs.getString(2))) throw new RuntimeException("Round " + round + " C: Wrong value");
                    }
                }
            }
        }
        System.out.println("Test 3 complete");

        // Test 4: Prepare without Execute (pure Describe flow)
        System.out.println("Test 4: Prepare without immediate Execute");
        try (Connection connection = DriverManager.getConnection(databaseUrl)) {
            // Prepare multiple statements without executing them
            List<PreparedStatement> cmds = new ArrayList<>();
            for (int i = 0; i < 5; i++) {
                PreparedStatement pstmt = connection.prepareStatement(
                    "SELECT * FROM test_describe_cached WHERE id = ?");
                cmds.add(pstmt);
            }

            // Now execute them all
            for (PreparedStatement pstmt : cmds) {
                pstmt.setInt(1, 1);
                try (ResultSet rs = pstmt.executeQuery()) {
                    // Just verify no errors
                }
                pstmt.close();
            }
        }
        System.out.println("Test 4 complete");

        // Test 5: Prepare on new connection (tests cross-connection caching)
        System.out.println("Test 5: Prepare across multiple connections");
        {
            String query = "SELECT * FROM test_describe_cached WHERE id = ?";

            for (int connNum = 0; connNum < 5; connNum++) {
                try (Connection connection = DriverManager.getConnection(databaseUrl);
                     PreparedStatement pstmt = connection.prepareStatement(query)) {
                    pstmt.setInt(1, 1);
                    try (ResultSet rs = pstmt.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("Conn " + connNum + ": No row");
                    }
                }
            }
        }
        System.out.println("Test 5 complete");

        System.out.println("describe_flow_cached complete");
    }
}
