import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;
import java.util.Random;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Prepared statements with large data test.
 * Tests handling of large result sets (>8KB) with prepared statements.
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
        config.setMaximumPoolSize(15);
        config.setMinimumIdle(1);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config)) {
            // Test 1: Large result set (>8196 bytes) with prepared statement
            System.out.println("Test 1: Large result set with prepared statement");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_large_data; " +
                        "CREATE TABLE test_large_data(id serial primary key, large_text text, data bytea)");
                }

                // Insert rows with large text (each row ~1KB, insert 20 rows = ~20KB total)
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_large_data(large_text, data) VALUES(?, ?)")) {
                    Random random = new Random();
                    for (int i = 0; i < 20; i++) {
                        char c = (char) ('A' + (i % 26));
                        String largeText = repeat(c, 1000) + "_row_" + i;
                        byte[] binaryData = new byte[500];
                        random.setSeed(i);
                        random.nextBytes(binaryData);

                        pstmt.setString(1, largeText);
                        pstmt.setBytes(2, binaryData);
                        pstmt.executeUpdate();
                    }
                }

                // Select all data (should be >8196 bytes)
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_large_data ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int totalBytes = 0;
                    int rowCount = 0;
                    while (rs.next()) {
                        String text = rs.getString(2);
                        byte[] data = rs.getBytes(3);
                        totalBytes += text.length() + data.length;
                        rowCount++;
                    }
                    if (rowCount != 20) throw new RuntimeException("Expected 20 rows, got " + rowCount);
                    if (totalBytes < 8196) throw new RuntimeException("Expected >8196 bytes, got " + totalBytes);
                    System.out.println("  Retrieved " + rowCount + " rows, " + totalBytes + " bytes total");
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Multiple parallel clients with prepared statements and large data
            System.out.println("Test 2: Parallel clients with large data");
            {
                ExecutorService executor = Executors.newFixedThreadPool(5);
                List<Future<?>> futures = new ArrayList<>();
                AtomicInteger errors = new AtomicInteger(0);

                for (int clientId = 0; clientId < 5; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            String tableName = "test_parallel_large_" + id;

                            try (Statement stmt = conn.createStatement()) {
                                stmt.execute("DROP TABLE IF EXISTS " + tableName + "; " +
                                    "CREATE TABLE " + tableName + "(id serial primary key, data text)");
                            }

                            // Insert large data
                            try (PreparedStatement pstmt = conn.prepareStatement(
                                    "INSERT INTO " + tableName + "(data) VALUES(?)")) {
                                for (int i = 0; i < 10; i++) {
                                    pstmt.setString(1, repeat((char) ('A' + id), 2000) + "_" + i);
                                    pstmt.executeUpdate();
                                }
                            }

                            // Select with prepared statement (>8196 bytes result)
                            try (PreparedStatement pstmt = conn.prepareStatement(
                                    "SELECT * FROM " + tableName + " ORDER BY id");
                                 ResultSet rs = pstmt.executeQuery()) {
                                int rows = 0;
                                int bytes = 0;
                                while (rs.next()) {
                                    bytes += rs.getString(2).length();
                                    rows++;
                                }
                                if (rows != 10) throw new RuntimeException("Client " + id + ": Expected 10 rows, got " + rows);
                                if (bytes < 8196) throw new RuntimeException("Client " + id + ": Expected >8196 bytes, got " + bytes);
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

                if (errors.get() > 0) throw new RuntimeException("Parallel test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 2 complete");

            // Test 3: Mixed prepared and unprepared queries with large results
            System.out.println("Test 3: Mixed prepared/unprepared with large results");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_mixed_large; " +
                        "CREATE TABLE test_mixed_large(id serial primary key, payload text)");
                }

                // Prepared insert
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_mixed_large(payload) VALUES(?)")) {
                    for (int i = 0; i < 15; i++) {
                        pstmt.setString(1, repeat('X', 1500) + "_prepared_" + i);
                        pstmt.executeUpdate();
                    }
                }

                // Unprepared insert
                for (int i = 0; i < 15; i++) {
                    try (Statement stmt = connection.createStatement()) {
                        stmt.execute("INSERT INTO test_mixed_large(payload) VALUES('" + repeat('Y', 1500) + "_unprepared_" + i + "')");
                    }
                }

                // Prepared select
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_mixed_large WHERE payload NOT LIKE '%unprepared%' ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    int bytes = 0;
                    while (rs.next()) {
                        bytes += rs.getString(2).length();
                        count++;
                    }
                    if (count != 15) throw new RuntimeException("Expected 15 prepared rows, got " + count);
                    System.out.println("  Prepared select: " + count + " rows, " + bytes + " bytes");
                }

                // Unprepared select
                try (Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT * FROM test_mixed_large ORDER BY id")) {
                    int count = 0;
                    int bytes = 0;
                    while (rs.next()) {
                        bytes += rs.getString(2).length();
                        count++;
                    }
                    if (count != 30) throw new RuntimeException("Expected 30 total rows, got " + count);
                    if (bytes < 8196) throw new RuntimeException("Expected >8196 bytes, got " + bytes);
                    System.out.println("  Unprepared select: " + count + " rows, " + bytes + " bytes");
                }
            }
            System.out.println("Test 3 complete");

            // Test 4: Concurrent prepared statements on same connection with large data
            System.out.println("Test 4: Concurrent prepared statements same connection");
            try (Connection connection = dataSource.getConnection()) {
                // Setup multiple tables
                for (int t = 0; t < 3; t++) {
                    try (Statement stmt = connection.createStatement()) {
                        stmt.execute("DROP TABLE IF EXISTS test_concurrent_prep_" + t + "; " +
                            "CREATE TABLE test_concurrent_prep_" + t + "(id serial primary key, val text)");
                    }
                }

                // Create and use multiple prepared statements
                List<PreparedStatement> preparedCmds = new ArrayList<>();
                for (int t = 0; t < 3; t++) {
                    preparedCmds.add(connection.prepareStatement(
                        "INSERT INTO test_concurrent_prep_" + t + "(val) VALUES(?)"));
                }

                // Interleave executions with large data
                for (int i = 0; i < 10; i++) {
                    for (int t = 0; t < 3; t++) {
                        preparedCmds.get(t).setString(1, repeat((char) ('A' + t), 1000) + "_iter_" + i);
                        preparedCmds.get(t).executeUpdate();
                    }
                }

                for (PreparedStatement pstmt : preparedCmds) {
                    pstmt.close();
                }

                // Verify
                for (int t = 0; t < 3; t++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_concurrent_prep_" + t + " ORDER BY id");
                         ResultSet rs = pstmt.executeQuery()) {
                        int count = 0;
                        int bytes = 0;
                        while (rs.next()) {
                            bytes += rs.getString(2).length();
                            count++;
                        }
                        if (count != 10) throw new RuntimeException("Table " + t + ": Expected 10 rows, got " + count);
                        if (bytes < 8196) throw new RuntimeException("Table " + t + ": Expected >8196 bytes, got " + bytes);
                    }
                }
            }
            System.out.println("Test 4 complete");

            // Test 5: Prepared statement with very large single row (>8196 bytes)
            System.out.println("Test 5: Very large single row");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_very_large; " +
                        "CREATE TABLE test_very_large(id serial primary key, huge_text text)");
                }

                // Insert single row with >10KB of data
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_very_large(huge_text) VALUES(?)")) {
                    StringBuilder hugeText = new StringBuilder();
                    for (int i = 0; i < 15; i++) {
                        hugeText.append(repeat((char) ('A' + i % 26), 1000));
                        hugeText.append("_block_").append(i).append("_");
                    }
                    pstmt.setString(1, hugeText.toString());
                    pstmt.executeUpdate();
                }

                // Select with prepared statement
                try (PreparedStatement pstmt = connection.prepareStatement("SELECT huge_text FROM test_very_large");
                     ResultSet rs = pstmt.executeQuery()) {
                    rs.next();
                    String result = rs.getString(1);
                    if (result.length() < 15000) throw new RuntimeException("Expected >15000 chars, got " + result.length());
                    System.out.println("  Retrieved single row with " + result.length() + " characters");
                }
            }
            System.out.println("Test 5 complete");

            // Test 6: Multiple connections with same prepared statement pattern
            System.out.println("Test 6: Multiple connections same prepared pattern");
            {
                ExecutorService executor = Executors.newFixedThreadPool(10);
                List<Future<?>> futures = new ArrayList<>();
                int[] results = new int[10];

                for (int i = 0; i < 10; i++) {
                    final int idx = i;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection();
                             PreparedStatement pstmt = conn.prepareStatement(
                                 "SELECT LENGTH(REPEAT(?, ?))")) {
                            pstmt.setString(1, repeat('Z', 100));
                            pstmt.setInt(2, 100 + idx * 10);
                            try (ResultSet rs = pstmt.executeQuery()) {
                                rs.next();
                                results[idx] = rs.getInt(1);
                            }
                        } catch (Exception e) {
                            throw new RuntimeException(e);
                        }
                    }));
                }

                for (Future<?> future : futures) {
                    future.get();
                }
                executor.shutdown();

                for (int i = 0; i < 10; i++) {
                    int expected = 100 * (100 + i * 10);
                    if (results[i] != expected) throw new RuntimeException("Connection " + i + ": Expected " + expected + ", got " + results[i]);
                }
            }
            System.out.println("Test 6 complete");

            // Test 7: Streaming large result with multiple DataRow messages
            System.out.println("Test 7: Streaming large result");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_stream_large; " +
                        "CREATE TABLE test_stream_large(id serial primary key, chunk text)");
                }

                // Insert 100 rows with ~500 bytes each = ~50KB total
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_stream_large(chunk) VALUES(?)")) {
                    for (int i = 0; i < 100; i++) {
                        pstmt.setString(1, repeat((char) ('A' + i % 26), 500) + "_" + String.format("%04d", i));
                        pstmt.executeUpdate();
                    }
                }

                // Stream all rows with prepared statement
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_stream_large ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int rowCount = 0;
                    long totalBytes = 0;
                    while (rs.next()) {
                        totalBytes += rs.getString(2).length();
                        rowCount++;
                    }
                    if (rowCount != 100) throw new RuntimeException("Expected 100 rows, got " + rowCount);
                    if (totalBytes < 50000) throw new RuntimeException("Expected >50000 bytes, got " + totalBytes);
                    System.out.println("  Streamed " + rowCount + " rows, " + totalBytes + " bytes");
                }
            }
            System.out.println("Test 7 complete");

            System.out.println("prepared_extended_large complete");
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
