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
 * Named prepared statements with Describe test.
 * Tests prepared statement behavior with explicit Prepare and large results.
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
            // Test 1: Named prepared statement with explicit Prepare and large result
            System.out.println("Test 1: Named prepared statement with Describe and large result");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_named_prep; " +
                        "CREATE TABLE test_named_prep(id serial primary key, name text, data text)");
                }

                // Insert large data
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_named_prep(name, data) VALUES(?, ?)")) {
                    for (int i = 0; i < 30; i++) {
                        pstmt.setString(1, "item_" + i);
                        pstmt.setString(2, repeat('X', 500) + "_" + i);
                        pstmt.executeUpdate();
                    }
                }

                // Select with prepared statement - result >8KB
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT id, name, data FROM test_named_prep WHERE id > ? ORDER BY id")) {
                    pstmt.setInt(1, 0);
                    int rowCount = 0;
                    int totalBytes = 0;
                    try (ResultSet rs = pstmt.executeQuery()) {
                        while (rs.next()) {
                            totalBytes += rs.getString(2).length() + rs.getString(3).length();
                            rowCount++;
                        }
                    }
                    if (rowCount != 30) throw new RuntimeException("Expected 30 rows, got " + rowCount);
                    System.out.println("  Retrieved " + rowCount + " rows, " + totalBytes + " bytes");
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Multiple prepared statements with interleaved execution and large data
            System.out.println("Test 2: Interleaved prepared statements with large data");
            try (Connection connection = dataSource.getConnection()) {
                PreparedStatement selectAll = connection.prepareStatement(
                    "SELECT * FROM test_named_prep ORDER BY id");
                PreparedStatement selectById = connection.prepareStatement(
                    "SELECT * FROM test_named_prep WHERE id = ?");
                PreparedStatement selectRange = connection.prepareStatement(
                    "SELECT * FROM test_named_prep WHERE id BETWEEN ? AND ? ORDER BY id");

                // Interleave executions with large results
                for (int iteration = 0; iteration < 5; iteration++) {
                    // Execute selectAll (large result >8KB)
                    int count1 = 0;
                    try (ResultSet rs = selectAll.executeQuery()) {
                        while (rs.next()) count1++;
                    }
                    if (count1 != 30) throw new RuntimeException("selectAll: Expected 30, got " + count1);

                    // Execute selectById (small result)
                    selectById.setInt(1, iteration + 1);
                    try (ResultSet rs = selectById.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("selectById: No row for id=" + (iteration + 1));
                    }

                    // Execute selectRange (medium result)
                    selectRange.setInt(1, 1);
                    selectRange.setInt(2, 15);
                    int count3 = 0;
                    try (ResultSet rs = selectRange.executeQuery()) {
                        while (rs.next()) count3++;
                    }
                    if (count3 != 15) throw new RuntimeException("selectRange: Expected 15, got " + count3);
                }

                selectAll.close();
                selectById.close();
                selectRange.close();
            }
            System.out.println("Test 2 complete");

            // Test 3: Prepared statement reuse across unprepared queries with large data
            System.out.println("Test 3: Mixed prepared/unprepared with large results");
            try (Connection connection = dataSource.getConnection()) {
                PreparedStatement prepared = connection.prepareStatement(
                    "SELECT * FROM test_named_prep WHERE id > ? ORDER BY id");

                for (int i = 0; i < 10; i++) {
                    // Execute prepared (large result)
                    prepared.setInt(1, 0);
                    int preparedCount = 0;
                    try (ResultSet rs = prepared.executeQuery()) {
                        while (rs.next()) preparedCount++;
                    }
                    if (preparedCount != 30) throw new RuntimeException("Prepared: Expected 30, got " + preparedCount);

                    // Execute unprepared query in between (also large result)
                    try (Statement stmt = connection.createStatement();
                         ResultSet rs = stmt.executeQuery("SELECT * FROM test_named_prep WHERE name LIKE 'item_%' ORDER BY id")) {
                        int unpreparedCount = 0;
                        while (rs.next()) unpreparedCount++;
                        if (unpreparedCount != 30) throw new RuntimeException("Unprepared: Expected 30, got " + unpreparedCount);
                    }
                }

                prepared.close();
            }
            System.out.println("Test 3 complete");

            // Test 4: Parallel connections with same prepared statement pattern
            System.out.println("Test 4: Parallel connections with prepared statements");
            {
                ExecutorService executor = Executors.newFixedThreadPool(8);
                List<Future<?>> futures = new ArrayList<>();
                AtomicInteger errors = new AtomicInteger(0);

                for (int clientId = 0; clientId < 8; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection();
                             PreparedStatement pstmt = conn.prepareStatement(
                                 "SELECT * FROM test_named_prep WHERE id > ? ORDER BY id")) {

                            for (int iter = 0; iter < 10; iter++) {
                                pstmt.setInt(1, id % 10);
                                int count = 0;
                                try (ResultSet rs = pstmt.executeQuery()) {
                                    while (rs.next()) count++;
                                }
                                int expected = 30 - (id % 10);
                                if (count != expected) {
                                    errors.incrementAndGet();
                                    System.err.println("Client " + id + " iter " + iter + ": Expected " + expected + ", got " + count);
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

                if (errors.get() > 0) throw new RuntimeException("Parallel test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 4 complete");

            // Test 5: Schema introspection with prepared statements
            System.out.println("Test 5: Schema introspection with prepared statements");
            try (Connection connection = dataSource.getConnection()) {
                // Get metadata
                ResultSet tables = connection.getMetaData().getTables(null, null, "test_named_prep", null);
                int tableCount = 0;
                while (tables.next()) tableCount++;
                tables.close();
                System.out.println("  Found " + tableCount + " tables");

                // Now execute prepared statement with large result
                try (PreparedStatement pstmt = connection.prepareStatement("SELECT * FROM test_named_prep ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    while (rs.next()) count++;
                    if (count != 30) throw new RuntimeException("Expected 30, got " + count);
                }

                // Get columns
                ResultSet columns = connection.getMetaData().getColumns(null, null, "test_named_prep", null);
                int colCount = 0;
                while (columns.next()) colCount++;
                columns.close();
                System.out.println("  Found " + colCount + " columns in test_named_prep");
            }
            System.out.println("Test 5 complete");

            // Test 6: Rapid prepare/execute/close cycle with large data
            System.out.println("Test 6: Rapid prepare/execute/close cycle");
            try (Connection connection = dataSource.getConnection()) {
                for (int cycle = 0; cycle < 20; cycle++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_named_prep WHERE id > ? ORDER BY id")) {
                        pstmt.setInt(1, cycle % 10);
                        int count = 0;
                        try (ResultSet rs = pstmt.executeQuery()) {
                            while (rs.next()) count++;
                        }
                        int expected = 30 - (cycle % 10);
                        if (count != expected) throw new RuntimeException("Cycle " + cycle + ": Expected " + expected + ", got " + count);
                    }
                }
            }
            System.out.println("Test 6 complete");

            // Test 7: Prepared INSERT followed by prepared SELECT with large data
            System.out.println("Test 7: Prepared INSERT then SELECT with large data");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_insert_select; " +
                        "CREATE TABLE test_insert_select(id serial primary key, payload text)");
                }

                // Prepared INSERT
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_insert_select(payload) VALUES(?)")) {
                    for (int i = 0; i < 50; i++) {
                        pstmt.setString(1, repeat((char) ('A' + (i % 26)), 300) + "_" + i);
                        pstmt.executeUpdate();
                    }
                }

                // Prepared SELECT immediately after (large result >8KB)
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_insert_select ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    int totalBytes = 0;
                    while (rs.next()) {
                        totalBytes += rs.getString(2).length();
                        count++;
                    }
                    if (count != 50) throw new RuntimeException("Expected 50, got " + count);
                    System.out.println("  Retrieved " + count + " rows, " + totalBytes + " bytes");
                }
            }
            System.out.println("Test 7 complete");

            // Test 8: Multiple prepared statements created before any execution
            System.out.println("Test 8: Batch prepare then batch execute");
            try (Connection connection = dataSource.getConnection()) {
                // Create all prepared statements first
                List<PreparedStatement> commands = new ArrayList<>();
                for (int i = 0; i < 5; i++) {
                    commands.add(connection.prepareStatement(
                        "SELECT * FROM test_named_prep WHERE id > ? ORDER BY id LIMIT " + ((i + 1) * 10)));
                }

                // Now execute all in various orders
                for (int round = 0; round < 3; round++) {
                    for (int i = commands.size() - 1; i >= 0; i--) {
                        commands.get(i).setInt(1, 0);
                        int count = 0;
                        try (ResultSet rs = commands.get(i).executeQuery()) {
                            while (rs.next()) count++;
                        }
                        int expected = Math.min((i + 1) * 10, 30);
                        if (count != expected) throw new RuntimeException("Round " + round + " cmd " + i + ": Expected " + expected + ", got " + count);
                    }
                }

                for (PreparedStatement pstmt : commands) {
                    pstmt.close();
                }
            }
            System.out.println("Test 8 complete");

            // Test 9: Prepared statement with NULL parameters and large result
            System.out.println("Test 9: NULL parameters with large result");
            try (Connection connection = dataSource.getConnection()) {
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_named_prep WHERE (? IS NULL OR name LIKE ?) ORDER BY id")) {

                    // First with NULL (returns all rows - large result)
                    pstmt.setNull(1, java.sql.Types.VARCHAR);
                    pstmt.setNull(2, java.sql.Types.VARCHAR);
                    int count1 = 0;
                    try (ResultSet rs = pstmt.executeQuery()) {
                        while (rs.next()) count1++;
                    }
                    if (count1 != 30) throw new RuntimeException("NULL filter: Expected 30, got " + count1);

                    // Then with actual filter
                    pstmt.setString(1, "item_1%");
                    pstmt.setString(2, "item_1%");
                    int count2 = 0;
                    try (ResultSet rs = pstmt.executeQuery()) {
                        while (rs.next()) count2++;
                    }
                    // item_1, item_10-19 = 11 rows
                    if (count2 != 11) throw new RuntimeException("Filter 'item_1%': Expected 11, got " + count2);
                }
            }
            System.out.println("Test 9 complete");

            // Test 10: Transaction with prepared statements and large data
            System.out.println("Test 10: Transaction with prepared statements");
            try (Connection connection = dataSource.getConnection()) {
                connection.setAutoCommit(false);

                // Prepared INSERT in transaction
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "INSERT INTO test_named_prep(name, data) VALUES(?, ?)")) {
                    for (int i = 0; i < 10; i++) {
                        pstmt.setString(1, "tx_item_" + i);
                        pstmt.setString(2, repeat('T', 500));
                        pstmt.executeUpdate();
                    }
                }

                // Prepared SELECT in same transaction (large result)
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_named_prep ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    while (rs.next()) count++;
                    if (count != 40) throw new RuntimeException("In transaction: Expected 40, got " + count);
                }

                connection.rollback(); // Don't actually commit

                // Verify rollback worked
                try (Statement stmt = connection.createStatement();
                     ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM test_named_prep")) {
                    rs.next();
                    long count = rs.getLong(1);
                    if (count != 30) throw new RuntimeException("After rollback: Expected 30, got " + count);
                }
            }
            System.out.println("Test 10 complete");

            System.out.println("prepared_named_describe complete");
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
