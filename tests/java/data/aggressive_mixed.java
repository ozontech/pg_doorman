import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;
import java.util.Random;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Aggressive mixed tests: batch + prepared statements + extended protocol.
 * These tests are intentionally aggressive and may expose server issues.
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
            // Test 1: Batch with prepared statements interleaved
            System.out.println("Test 1: Batch with prepared statements interleaved");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_aggressive_mixed; " +
                        "CREATE TABLE test_aggressive_mixed(id serial primary key, value int, data text)");
                }
                connection.commit();

                for (int round = 0; round < 20; round++) {
                    // Batch insert
                    try (Statement stmt = connection.createStatement()) {
                        stmt.addBatch("INSERT INTO test_aggressive_mixed(value, data) VALUES (" + (round * 10 + 1) + ", 'batch1')");
                        stmt.addBatch("INSERT INTO test_aggressive_mixed(value, data) VALUES (" + (round * 10 + 2) + ", 'batch2')");
                        stmt.addBatch("INSERT INTO test_aggressive_mixed(value, data) VALUES (" + (round * 10 + 3) + ", 'batch3')");
                        stmt.executeBatch();
                    }
                    connection.commit();

                    // Prepared statement select
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_aggressive_mixed WHERE value > ? ORDER BY id")) {
                        pstmt.setInt(1, round * 10);
                        int count = 0;
                        try (ResultSet rs = pstmt.executeQuery()) {
                            while (rs.next()) count++;
                        }
                        if (count != 3) throw new RuntimeException("Round " + round + ": Expected 3, got " + count);
                    }

                    // Cleanup for next round
                    try (Statement stmt = connection.createStatement()) {
                        stmt.execute("DELETE FROM test_aggressive_mixed");
                    }
                    connection.commit();
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Parallel batch and prepared statements from multiple connections
            System.out.println("Test 2: Parallel batch and prepared statements");
            {
                AtomicInteger errors = new AtomicInteger(0);
                CyclicBarrier barrier = new CyclicBarrier(12);
                ExecutorService executor = Executors.newFixedThreadPool(12);
                List<Future<?>> futures = new ArrayList<>();

                // Setup shared table
                try (Connection setupConn = dataSource.getConnection();
                     Statement stmt = setupConn.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_parallel_mixed; " +
                        "CREATE TABLE test_parallel_mixed(id serial primary key, client_id int, round int, data text)");
                }

                for (int clientId = 0; clientId < 12; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            conn.setAutoCommit(false);
                            for (int round = 0; round < 15; round++) {
                                barrier.await();

                                if (id % 3 == 0) {
                                    // Batch operations
                                    try (Statement stmt = conn.createStatement()) {
                                        stmt.addBatch("INSERT INTO test_parallel_mixed(client_id, round, data) VALUES (" + id + ", " + round + ", 'batch_data')");
                                        stmt.executeBatch();
                                    }
                                    conn.commit();

                                    try (PreparedStatement pstmt = conn.prepareStatement(
                                            "SELECT * FROM test_parallel_mixed WHERE client_id = ? AND round = ?")) {
                                        pstmt.setInt(1, id);
                                        pstmt.setInt(2, round);
                                        int count = 0;
                                        try (ResultSet rs = pstmt.executeQuery()) {
                                            while (rs.next()) count++;
                                        }
                                        if (count != 1) {
                                            errors.incrementAndGet();
                                            System.err.println("Client " + id + " round " + round + ": batch select expected 1 row, got " + count);
                                        }
                                    }
                                } else if (id % 3 == 1) {
                                    // Prepared statements
                                    try (PreparedStatement pstmt = conn.prepareStatement(
                                            "INSERT INTO test_parallel_mixed(client_id, round, data) VALUES (?, ?, ?)")) {
                                        pstmt.setInt(1, id);
                                        pstmt.setInt(2, round);
                                        pstmt.setString(3, "prepared_data");
                                        pstmt.executeUpdate();
                                    }
                                    conn.commit();

                                    try (PreparedStatement pstmt = conn.prepareStatement(
                                            "SELECT * FROM test_parallel_mixed WHERE client_id = ? AND round = ?")) {
                                        pstmt.setInt(1, id);
                                        pstmt.setInt(2, round);
                                        try (ResultSet rs = pstmt.executeQuery()) {
                                            if (!rs.next()) {
                                                errors.incrementAndGet();
                                                System.err.println("Client " + id + " round " + round + ": prepared select returned no rows");
                                            }
                                        }
                                    }
                                } else {
                                    // Mixed: batch insert + prepared select
                                    try (Statement stmt = conn.createStatement()) {
                                        stmt.addBatch("INSERT INTO test_parallel_mixed(client_id, round, data) VALUES (" + id + ", " + round + ", 'mixed_data')");
                                        stmt.executeBatch();
                                    }
                                    conn.commit();

                                    try (PreparedStatement pstmt = conn.prepareStatement(
                                            "SELECT * FROM test_parallel_mixed WHERE client_id = ? AND round = ?")) {
                                        pstmt.setInt(1, id);
                                        pstmt.setInt(2, round);
                                        try (ResultSet rs = pstmt.executeQuery()) {
                                            if (!rs.next()) {
                                                errors.incrementAndGet();
                                                System.err.println("Client " + id + " round " + round + ": mixed select returned no rows");
                                            }
                                        }
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

                if (errors.get() > 0) throw new RuntimeException("Parallel mixed test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 2 complete");

            // Test 3: Rapid batch/prepared switching on single connection
            System.out.println("Test 3: Rapid batch/prepared switching");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_rapid_switch; " +
                        "CREATE TABLE test_rapid_switch(id serial primary key, val int)");
                }
                connection.commit();

                for (int i = 0; i < 100; i++) {
                    if (i % 2 == 0) {
                        // Batch
                        try (Statement stmt = connection.createStatement()) {
                            stmt.addBatch("INSERT INTO test_rapid_switch(val) VALUES (" + i + ")");
                            stmt.executeBatch();
                        }
                        connection.commit();
                    } else {
                        // Prepared
                        try (PreparedStatement pstmt = connection.prepareStatement(
                                "INSERT INTO test_rapid_switch(val) VALUES (?)")) {
                            pstmt.setInt(1, i);
                            pstmt.executeUpdate();
                        }
                        connection.commit();
                    }

                    // Verify count
                    try (Statement stmt = connection.createStatement();
                         ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_rapid_switch")) {
                        rs.next();
                        long count = rs.getLong(1);
                        if (count != i + 1) throw new RuntimeException("Iter " + i + ": Expected " + (i + 1) + ", got " + count);
                    }
                }
            }
            System.out.println("Test 3 complete");

            // Test 4: Large batch with prepared statements in between
            System.out.println("Test 4: Large batch with prepared statements in between");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_large_batch; " +
                        "CREATE TABLE test_large_batch(id serial primary key, data text)");
                }
                connection.commit();

                for (int round = 0; round < 10; round++) {
                    // Large batch with 20 commands
                    try (Statement stmt = connection.createStatement()) {
                        for (int i = 0; i < 20; i++) {
                            stmt.addBatch("INSERT INTO test_large_batch(data) VALUES ('" + repeat('X', 500) + "')");
                        }
                        stmt.executeBatch();
                    }
                    connection.commit();

                    // Prepared statement in between
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT COUNT(*) FROM test_large_batch")) {
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            long count = rs.getLong(1);
                            if (count != (round + 1) * 20) throw new RuntimeException("Round " + round + ": Expected " + ((round + 1) * 20) + ", got " + count);
                        }
                    }
                }
            }
            System.out.println("Test 4 complete");

            // Test 5: Batch with parameters (prepared batch)
            System.out.println("Test 5: Batch with parameters");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_batch_params; " +
                        "CREATE TABLE test_batch_params(id serial primary key, a int, b text)");
                }
                connection.commit();

                for (int round = 0; round < 20; round++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "INSERT INTO test_batch_params(a, b) VALUES (?, ?)")) {
                        for (int i = 0; i < 5; i++) {
                            pstmt.setInt(1, round * 5 + i);
                            pstmt.setString(2, "data_" + round + "_" + i);
                            pstmt.addBatch();
                        }
                        pstmt.executeBatch();
                    }
                    connection.commit();

                    // Verify with prepared statement
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT COUNT(*) FROM test_batch_params WHERE a >= ? AND a < ?")) {
                        pstmt.setInt(1, round * 5);
                        pstmt.setInt(2, (round + 1) * 5);
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            long count = rs.getLong(1);
                            if (count != 5) throw new RuntimeException("Round " + round + ": Expected 5, got " + count);
                        }
                    }
                }
            }
            System.out.println("Test 5 complete");

            // Test 6: Concurrent batch operations with transaction isolation
            System.out.println("Test 6: Concurrent batch with transactions");
            {
                AtomicInteger errors = new AtomicInteger(0);
                ExecutorService executor = Executors.newFixedThreadPool(8);
                List<Future<?>> futures = new ArrayList<>();

                try (Connection setupConn = dataSource.getConnection();
                     Statement stmt = setupConn.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_batch_tx; " +
                        "CREATE TABLE test_batch_tx(id serial primary key, client_id int, value int)");
                }

                for (int clientId = 0; clientId < 8; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            for (int round = 0; round < 10; round++) {
                                conn.setAutoCommit(false);

                                // Batch insert within transaction
                                try (Statement stmt = conn.createStatement()) {
                                    for (int i = 0; i < 5; i++) {
                                        stmt.addBatch("INSERT INTO test_batch_tx(client_id, value) VALUES (" + id + ", " + (round * 5 + i) + ")");
                                    }
                                    stmt.executeBatch();
                                }

                                // Prepared select within same transaction
                                try (PreparedStatement pstmt = conn.prepareStatement(
                                        "SELECT COUNT(*) FROM test_batch_tx WHERE client_id = ?")) {
                                    pstmt.setInt(1, id);
                                    try (ResultSet rs = pstmt.executeQuery()) {
                                        rs.next();
                                        long count = rs.getLong(1);
                                        long expected = (round + 1) * 5;
                                        if (count != expected) {
                                            errors.incrementAndGet();
                                            System.err.println("Client " + id + " round " + round + ": Expected " + expected + ", got " + count);
                                        }
                                    }
                                }

                                conn.commit();
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

                if (errors.get() > 0) throw new RuntimeException("Concurrent batch tx test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 6 complete");

            // Test 7: Stress test - rapid fire batch + prepared + simple queries
            System.out.println("Test 7: Rapid fire mixed queries stress test");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_stress_mixed; " +
                        "CREATE TABLE test_stress_mixed(id serial primary key, t text)");
                }
                connection.commit();

                Random random = new Random(42);

                for (int i = 0; i < 200; i++) {
                    int choice = random.nextInt(4);

                    switch (choice) {
                        case 0:
                            // Simple query
                            try (Statement stmt = connection.createStatement()) {
                                stmt.execute("INSERT INTO test_stress_mixed(t) VALUES ('simple_" + i + "')");
                            }
                            break;

                        case 1:
                            // Prepared statement
                            try (PreparedStatement pstmt = connection.prepareStatement(
                                    "INSERT INTO test_stress_mixed(t) VALUES (?)")) {
                                pstmt.setString(1, "prepared_" + i);
                                pstmt.executeUpdate();
                            }
                            break;

                        case 2:
                            // Batch insert
                            try (Statement stmt = connection.createStatement()) {
                                stmt.addBatch("INSERT INTO test_stress_mixed(t) VALUES ('batch_" + i + "_a')");
                                stmt.addBatch("INSERT INTO test_stress_mixed(t) VALUES ('batch_" + i + "_b')");
                                stmt.executeBatch();
                            }
                            break;

                        case 3:
                            // Prepared batch
                            try (PreparedStatement pstmt = connection.prepareStatement(
                                    "INSERT INTO test_stress_mixed(t) VALUES (?)")) {
                                pstmt.setString(1, "param_batch_" + i);
                                pstmt.addBatch();
                                pstmt.executeBatch();
                            }
                            break;
                    }
                    connection.commit();

                    // Periodically verify count
                    if (i % 50 == 49) {
                        try (PreparedStatement pstmt = connection.prepareStatement(
                                "SELECT COUNT(*) FROM test_stress_mixed");
                             ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            long c = rs.getLong(1);
                            if (c < i) throw new RuntimeException("Iter " + i + ": Count " + c + " is less than expected minimum " + i);
                        }
                    }
                }
            }
            System.out.println("Test 7 complete");

            // Test 8: Batch with errors - partial failure handling
            System.out.println("Test 8: Batch with errors handling");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_batch_errors; " +
                        "CREATE TABLE test_batch_errors(id serial primary key, val int UNIQUE)");
                }
                connection.commit();

                for (int round = 0; round < 10; round++) {
                    // First batch - should succeed
                    try (Statement stmt = connection.createStatement()) {
                        stmt.addBatch("INSERT INTO test_batch_errors(val) VALUES (" + (round * 100 + 1) + ")");
                        stmt.addBatch("INSERT INTO test_batch_errors(val) VALUES (" + (round * 100 + 2) + ")");
                        stmt.executeBatch();
                    }
                    connection.commit();

                    // Prepared statement after batch
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT COUNT(*) FROM test_batch_errors")) {
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            long count = rs.getLong(1);
                            if (count != (round + 1) * 2) throw new RuntimeException("Round " + round + ": Expected " + ((round + 1) * 2) + ", got " + count);
                        }
                    }

                    // Batch that will fail (duplicate key)
                    try (Statement stmt = connection.createStatement()) {
                        stmt.addBatch("INSERT INTO test_batch_errors(val) VALUES (" + (round * 100 + 1) + ")"); // duplicate!
                        stmt.executeBatch();
                        connection.commit();
                        throw new RuntimeException("Round " + round + ": Expected duplicate key error");
                    } catch (SQLException ex) {
                        if (!"23505".equals(ex.getSQLState())) {
                            throw new RuntimeException("Expected unique_violation (23505), got: " + ex.getSQLState());
                        }
                        connection.rollback();
                    }

                    // Connection should still work after error
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT COUNT(*) FROM test_batch_errors")) {
                        try (ResultSet rs = pstmt.executeQuery()) {
                            rs.next();
                            long count = rs.getLong(1);
                            if (count != (round + 1) * 2) throw new RuntimeException("Round " + round + " after error: Expected " + ((round + 1) * 2) + ", got " + count);
                        }
                    }
                }
            }
            System.out.println("Test 8 complete");

            // Test 9: Extended protocol with metadata interleaved with batch
            System.out.println("Test 9: Extended protocol metadata with batch");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);
                
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_describe_batch; " +
                        "CREATE TABLE test_describe_batch(id serial primary key, col1 text, col2 int, col3 boolean, col4 timestamp)");
                }
                connection.commit();

                for (int round = 0; round < 15; round++) {
                    // Batch insert
                    try (Statement stmt = connection.createStatement()) {
                        stmt.addBatch("INSERT INTO test_describe_batch(col1, col2, col3, col4) VALUES ('test', " + round + ", true, NOW())");
                        stmt.executeBatch();
                    }
                    connection.commit();

                    // Get metadata (triggers Describe-like behavior)
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_describe_batch")) {
                        var meta = pstmt.getMetaData();
                        if (meta.getColumnCount() != 5) throw new RuntimeException("Round " + round + ": Expected 5 columns in schema");
                    }

                    // Prepared with explicit columns
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT col1, col2 FROM test_describe_batch WHERE id = ?")) {
                        pstmt.setInt(1, round + 1);
                        try (ResultSet rs = pstmt.executeQuery()) {
                            if (!rs.next()) throw new RuntimeException("Round " + round + ": No row found");
                            var meta = rs.getMetaData();
                            if (meta.getColumnCount() != 2) throw new RuntimeException("Round " + round + ": Expected 2 columns, got " + meta.getColumnCount());
                        }
                    }
                }
            }
            System.out.println("Test 9 complete");

            // Test 10: Maximum stress - all patterns combined with high concurrency
            System.out.println("Test 10: Maximum stress combined patterns");
            {
                AtomicInteger errors = new AtomicInteger(0);
                CyclicBarrier barrier = new CyclicBarrier(20);
                ExecutorService executor = Executors.newFixedThreadPool(20);
                List<Future<?>> futures = new ArrayList<>();

                try (Connection setupConn = dataSource.getConnection();
                     Statement stmt = setupConn.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_max_stress; " +
                        "CREATE TABLE test_max_stress(id serial primary key, client_id int, round int, pattern text, data text)");
                }

                for (int clientId = 0; clientId < 20; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            conn.setAutoCommit(false);
                            for (int round = 0; round < 20; round++) {
                                barrier.await();

                                int pattern = (id + round) % 5;

                                switch (pattern) {
                                    case 0:
                                        // Simple batch
                                        try (Statement stmt = conn.createStatement()) {
                                            stmt.addBatch("INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (" + id + ", " + round + ", 'batch', 'data')");
                                            stmt.executeBatch();
                                        }
                                        conn.commit();
                                        try (PreparedStatement pstmt = conn.prepareStatement(
                                                "SELECT * FROM test_max_stress WHERE client_id = ? AND round = ?")) {
                                            pstmt.setInt(1, id);
                                            pstmt.setInt(2, round);
                                            int count = 0;
                                            try (ResultSet rs = pstmt.executeQuery()) {
                                                while (rs.next()) count++;
                                            }
                                            if (count != 1) {
                                                errors.incrementAndGet();
                                                System.err.println("Client " + id + " round " + round + " pattern 0: expected 1 row, got " + count);
                                            }
                                        }
                                        break;

                                    case 1:
                                        // Prepared only
                                        try (PreparedStatement pstmt = conn.prepareStatement(
                                                "INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (?, ?, ?, ?)")) {
                                            pstmt.setInt(1, id);
                                            pstmt.setInt(2, round);
                                            pstmt.setString(3, "prepared");
                                            pstmt.setString(4, repeat('P', 100));
                                            pstmt.executeUpdate();
                                        }
                                        conn.commit();
                                        break;

                                    case 2:
                                        // Prepared batch
                                        try (PreparedStatement pstmt = conn.prepareStatement(
                                                "INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (?, ?, ?, ?)")) {
                                            pstmt.setInt(1, id);
                                            pstmt.setInt(2, round);
                                            pstmt.setString(3, "param_batch");
                                            pstmt.setString(4, repeat('B', 100));
                                            pstmt.addBatch();
                                            pstmt.executeBatch();
                                        }
                                        conn.commit();
                                        break;

                                    case 3:
                                        // Mixed: batch insert + prepared select
                                        try (Statement stmt = conn.createStatement()) {
                                            stmt.addBatch("INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (" + id + ", " + round + ", 'mixed', 'data')");
                                            stmt.executeBatch();
                                        }
                                        conn.commit();
                                        try (PreparedStatement pstmt = conn.prepareStatement(
                                                "SELECT * FROM test_max_stress WHERE client_id = ? AND round = ?")) {
                                            pstmt.setInt(1, id);
                                            pstmt.setInt(2, round);
                                            try (ResultSet rs = pstmt.executeQuery()) {
                                                if (!rs.next()) {
                                                    errors.incrementAndGet();
                                                    System.err.println("Client " + id + " round " + round + " pattern 3: no row");
                                                }
                                            }
                                        }
                                        break;

                                    case 4:
                                        // Transaction with batch and prepared
                                        try (Statement stmt = conn.createStatement()) {
                                            stmt.addBatch("INSERT INTO test_max_stress(client_id, round, pattern, data) VALUES (" + id + ", " + round + ", 'tx_batch', 'data')");
                                            stmt.executeBatch();
                                        }
                                        try (PreparedStatement pstmt = conn.prepareStatement(
                                                "SELECT COUNT(*) FROM test_max_stress WHERE client_id = ?")) {
                                            pstmt.setInt(1, id);
                                            try (ResultSet rs = pstmt.executeQuery()) {
                                                rs.next();
                                                long count = rs.getLong(1);
                                                if (count < 1) {
                                                    errors.incrementAndGet();
                                                    System.err.println("Client " + id + " round " + round + " pattern 4: count " + count);
                                                }
                                            }
                                        }
                                        conn.commit();
                                        break;
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

                if (errors.get() > 0) throw new RuntimeException("Maximum stress test failed with " + errors.get() + " errors");

                // Final verification
                try (Connection verifyConn = dataSource.getConnection();
                     Statement stmt = verifyConn.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_max_stress")) {
                    rs.next();
                    long total = rs.getLong(1);
                    // 20 clients * 20 rounds = 400 rows expected
                    if (total != 400) throw new RuntimeException("Final count: Expected 400, got " + total);
                }
            }
            System.out.println("Test 10 complete");

            System.out.println("aggressive_mixed complete");
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        }
    }

    private static String repeat(char c, int count) {
        StringBuilder sb = new StringBuilder(count);
        for (int i = 0; i < count; i++) {
            sb.append(c);
        }
        return sb.toString();
    }
}
