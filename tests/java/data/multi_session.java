import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Multi-session test.
 * Tests multiple sequential and parallel connections, connection reuse,
 * interleaved operations, and concurrent transactions.
 */
public class Main {
    private static String databaseUrl;

    public static void main(String[] args) {
        databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        HikariConfig config = new HikariConfig();
        config.setJdbcUrl(databaseUrl);
        config.setMaximumPoolSize(25);
        config.setMinimumIdle(1);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config)) {
            // Test 1: Multiple sequential connections
            System.out.println("Test 1: Multiple sequential connections");
            for (int i = 0; i < 10; i++) {
                try (Connection connection = dataSource.getConnection();
                     Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT " + i + " as val")) {
                    rs.next();
                    int result = rs.getInt(1);
                    if (result != i) throw new RuntimeException("Expected " + i + ", got " + result);
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Multiple parallel connections
            System.out.println("Test 2: Multiple parallel connections");
            {
                int[] results = new int[20];
                ExecutorService executor = Executors.newFixedThreadPool(20);
                List<Future<?>> futures = new ArrayList<>();

                for (int i = 0; i < 20; i++) {
                    final int index = i;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection();
                             Statement stmt = conn.createStatement();
                             ResultSet rs = stmt.executeQuery("SELECT " + index + " as val")) {
                            rs.next();
                            results[index] = rs.getInt(1);
                        } catch (Exception e) {
                            throw new RuntimeException(e);
                        }
                    }));
                }

                for (Future<?> future : futures) {
                    future.get();
                }
                executor.shutdown();

                for (int i = 0; i < 20; i++) {
                    if (results[i] != i) throw new RuntimeException("Parallel test failed: expected " + i + ", got " + results[i]);
                }
            }
            System.out.println("Test 2 complete");

            // Test 3: Connection reuse with different queries
            System.out.println("Test 3: Connection reuse");
            try (Connection connection = dataSource.getConnection()) {
                for (int i = 0; i < 50; i++) {
                    try (Statement stmt = connection.createStatement();
                         ResultSet rs = stmt.executeQuery("SELECT " + i + " * 2 as doubled")) {
                        rs.next();
                        int result = rs.getInt(1);
                        if (result != i * 2) throw new RuntimeException("Expected " + (i * 2) + ", got " + result);
                    }
                }
            }
            System.out.println("Test 3 complete");

            // Test 4: Interleaved connections with shared table
            System.out.println("Test 4: Interleaved connections");
            try (Connection setupConn = dataSource.getConnection();
                 Statement stmt = setupConn.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_interleaved; CREATE TABLE test_interleaved(id serial, session_id int, val int)");
            }

            try (Connection conn1 = dataSource.getConnection();
                 Connection conn2 = dataSource.getConnection()) {

                // Interleave operations
                for (int i = 0; i < 10; i++) {
                    try (PreparedStatement pstmt1 = conn1.prepareStatement("INSERT INTO test_interleaved(session_id, val) VALUES(1, ?)")) {
                        pstmt1.setInt(1, i);
                        pstmt1.executeUpdate();
                    }

                    try (PreparedStatement pstmt2 = conn2.prepareStatement("INSERT INTO test_interleaved(session_id, val) VALUES(2, ?)")) {
                        pstmt2.setInt(1, i * 10);
                        pstmt2.executeUpdate();
                    }
                }

                // Verify results
                try (Statement stmt = conn1.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_interleaved")) {
                    rs.next();
                    long count = rs.getLong(1);
                    if (count != 20) throw new RuntimeException("Expected 20 rows, got " + count);
                }

                try (Statement stmt = conn1.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT SUM(val) FROM test_interleaved WHERE session_id = 1")) {
                    rs.next();
                    long sum = rs.getLong(1);
                    if (sum != 45) throw new RuntimeException("Expected sum 45 for session 1, got " + sum);
                }

                try (Statement stmt = conn2.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT SUM(val) FROM test_interleaved WHERE session_id = 2")) {
                    rs.next();
                    long sum = rs.getLong(1);
                    if (sum != 450) throw new RuntimeException("Expected sum 450 for session 2, got " + sum);
                }
            }
            System.out.println("Test 4 complete");

            // Test 5: Prepared statements across multiple connections
            System.out.println("Test 5: Prepared statements across connections");
            {
                ExecutorService executor = Executors.newFixedThreadPool(5);
                List<Future<?>> futures = new ArrayList<>();
                AtomicInteger errors = new AtomicInteger(0);

                for (int connIdx = 0; connIdx < 5; connIdx++) {
                    final int idx = connIdx;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection();
                             PreparedStatement pstmt = conn.prepareStatement("SELECT ?::int + ?::int")) {

                            for (int i = 0; i < 20; i++) {
                                pstmt.setInt(1, idx);
                                pstmt.setInt(2, i);
                                try (ResultSet rs = pstmt.executeQuery()) {
                                    rs.next();
                                    int result = rs.getInt(1);
                                    if (result != idx + i) {
                                        errors.incrementAndGet();
                                        System.err.println("Conn " + idx + ": expected " + (idx + i) + ", got " + result);
                                    }
                                }
                            }
                        } catch (Exception e) {
                            errors.incrementAndGet();
                            e.printStackTrace();
                        }
                    }));
                }

                for (Future<?> future : futures) {
                    future.get();
                }
                executor.shutdown();

                if (errors.get() > 0) throw new RuntimeException("Test 5 failed with " + errors.get() + " errors");
            }
            System.out.println("Test 5 complete");

            // Test 6: Rapid connect/disconnect cycles
            System.out.println("Test 6: Rapid connect/disconnect");
            for (int i = 0; i < 30; i++) {
                try (Connection connection = dataSource.getConnection();
                     Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT 1")) {
                    rs.next();
                }
            }
            System.out.println("Test 6 complete");

            // Test 7: Long-running connection with periodic queries
            System.out.println("Test 7: Long-running connection");
            try (Connection connection = dataSource.getConnection()) {
                for (int i = 0; i < 20; i++) {
                    try (PreparedStatement pstmt = connection.prepareStatement("SELECT pg_sleep(0.01), ?::int as iteration")) {
                        pstmt.setInt(1, i);
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            int iteration = rs.getInt(2);
                            if (iteration != i) throw new RuntimeException("Expected iteration " + i + ", got " + iteration);
                        }
                    }
                }
            }
            System.out.println("Test 7 complete");

            // Test 8: Concurrent transactions on different connections
            System.out.println("Test 8: Concurrent transactions");
            try (Connection setupConn = dataSource.getConnection();
                 Statement stmt = setupConn.createStatement()) {
                stmt.execute("DROP TABLE IF EXISTS test_concurrent_tx; CREATE TABLE test_concurrent_tx(id int PRIMARY KEY, val int)");
            }

            {
                ExecutorService executor = Executors.newFixedThreadPool(5);
                List<Future<?>> futures = new ArrayList<>();
                AtomicInteger errors = new AtomicInteger(0);

                for (int txIdx = 0; txIdx < 5; txIdx++) {
                    final int idx = txIdx;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            conn.setAutoCommit(false);

                            try (Statement stmt = conn.createStatement()) {
                                stmt.execute("INSERT INTO test_concurrent_tx(id, val) VALUES(" + (idx * 100) + ", " + idx + ")");
                                stmt.execute("INSERT INTO test_concurrent_tx(id, val) VALUES(" + (idx * 100 + 1) + ", " + (idx + 10) + ")");
                            }

                            conn.commit();
                        } catch (Exception e) {
                            errors.incrementAndGet();
                            e.printStackTrace();
                        }
                    }));
                }

                for (Future<?> future : futures) {
                    future.get();
                }
                executor.shutdown();

                if (errors.get() > 0) throw new RuntimeException("Test 8 failed with " + errors.get() + " errors");
            }

            try (Connection verifyConn = dataSource.getConnection();
                 Statement stmt = verifyConn.createStatement();
                 ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_concurrent_tx")) {
                rs.next();
                long count = rs.getLong(1);
                if (count != 10) throw new RuntimeException("Expected 10 rows from concurrent transactions, got " + count);
            }
            System.out.println("Test 8 complete");

            System.out.println("multi_session complete");
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }
}
