import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.ResultSetMetaData;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Prepared statements stress test with Describe.
 * Tests rapid Describe requests, parallel Prepare, and extended protocol stress.
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
        config.setMaximumPoolSize(20);
        config.setMinimumIdle(1);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config)) {
            // Test 1: Rapid Describe requests with large results
            System.out.println("Test 1: Rapid Describe with large results");
            try (Connection connection = dataSource.getConnection()) {
                try (Statement stmt = connection.createStatement()) {
                    stmt.execute("DROP TABLE IF EXISTS test_stress_describe; " +
                        "CREATE TABLE test_stress_describe(" +
                        "id serial primary key, " +
                        "col1 text, col2 text, col3 text, col4 text, col5 text, " +
                        "col6 text, col7 text, col8 text, col9 text, col10 text)");
                }

                // Insert rows with large data
                for (int i = 0; i < 10; i++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "INSERT INTO test_stress_describe(col1, col2, col3, col4, col5, col6, col7, col8, col9, col10) " +
                            "VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")) {
                        for (int c = 1; c <= 10; c++) {
                            pstmt.setString(c, repeat((char) ('A' + c), 500));
                        }
                        pstmt.executeUpdate();
                    }
                }

                // Rapid prepare/execute cycle
                for (int cycle = 0; cycle < 50; cycle++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe WHERE id > ? ORDER BY id")) {
                        pstmt.setInt(1, 0);
                        int count = 0;
                        try (ResultSet rs = pstmt.executeQuery()) {
                            while (rs.next()) count++;
                        }
                        if (count != 10) throw new RuntimeException("Cycle " + cycle + ": Expected 10, got " + count);
                    }
                }
            }
            System.out.println("Test 1 complete");

            // Test 2: Parallel clients all doing Prepare simultaneously
            System.out.println("Test 2: Parallel Prepare stress test");
            {
                AtomicInteger errors = new AtomicInteger(0);
                CyclicBarrier barrier = new CyclicBarrier(16);
                ExecutorService executor = Executors.newFixedThreadPool(16);
                List<Future<?>> futures = new ArrayList<>();

                for (int clientId = 0; clientId < 16; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            for (int round = 0; round < 10; round++) {
                                barrier.await();

                                try (PreparedStatement pstmt = conn.prepareStatement(
                                        "SELECT * FROM test_stress_describe WHERE id > ? ORDER BY id")) {
                                    pstmt.setInt(1, id % 5);
                                    int count = 0;
                                    try (ResultSet rs = pstmt.executeQuery()) {
                                        while (rs.next()) count++;
                                    }
                                    int expected = 10 - (id % 5);
                                    if (count != expected) {
                                        errors.incrementAndGet();
                                        System.err.println("Client " + id + " round " + round + ": Expected " + expected + ", got " + count);
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

                if (errors.get() > 0) throw new RuntimeException("Parallel prepare test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 2 complete");

            // Test 3: Interleaved Prepare and Execute with different query patterns
            System.out.println("Test 3: Interleaved Prepare/Execute patterns");
            try (Connection connection = dataSource.getConnection()) {
                for (int iteration = 0; iteration < 20; iteration++) {
                    PreparedStatement cmdA = connection.prepareStatement(
                        "SELECT col1, col2, col3 FROM test_stress_describe WHERE id = ?");
                    PreparedStatement cmdB = connection.prepareStatement(
                        "SELECT col4, col5, col6, col7 FROM test_stress_describe WHERE id > ?");
                    PreparedStatement cmdC = connection.prepareStatement(
                        "SELECT * FROM test_stress_describe ORDER BY id");

                    // Execute in different order than prepared
                    int countC = 0;
                    try (ResultSet rs = cmdC.executeQuery()) {
                        while (rs.next()) countC++;
                    }
                    if (countC != 10) throw new RuntimeException("Iter " + iteration + " C: Expected 10, got " + countC);

                    cmdA.setInt(1, 1);
                    try (ResultSet rs = cmdA.executeQuery()) {
                        if (!rs.next()) throw new RuntimeException("Iter " + iteration + " A: No row");
                        ResultSetMetaData meta = rs.getMetaData();
                        if (meta.getColumnCount() != 3) throw new RuntimeException("Iter " + iteration + " A: Expected 3 columns, got " + meta.getColumnCount());
                    }

                    cmdB.setInt(1, 5);
                    int countB = 0;
                    try (ResultSet rs = cmdB.executeQuery()) {
                        while (rs.next()) {
                            ResultSetMetaData meta = rs.getMetaData();
                            if (meta.getColumnCount() != 4) throw new RuntimeException("Iter " + iteration + " B: Expected 4 columns, got " + meta.getColumnCount());
                            countB++;
                        }
                    }
                    if (countB != 5) throw new RuntimeException("Iter " + iteration + " B: Expected 5, got " + countB);

                    cmdA.close();
                    cmdB.close();
                    cmdC.close();
                }
            }
            System.out.println("Test 3 complete");

            // Test 4: SchemaOnly Describe test (using metadata)
            System.out.println("Test 4: SchemaOnly Describe test");
            try (Connection connection = dataSource.getConnection()) {
                for (int i = 0; i < 30; i++) {
                    // Get metadata (triggers Describe-like behavior)
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe WHERE id > ?")) {
                        pstmt.setInt(1, 0);
                        ResultSetMetaData meta = pstmt.getMetaData();
                        if (meta.getColumnCount() != 11) throw new RuntimeException("Iter " + i + ": Expected 11 columns, got " + meta.getColumnCount());
                    }

                    // Immediately follow with full query
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe ORDER BY id");
                         ResultSet rs = pstmt.executeQuery()) {
                        int count = 0;
                        while (rs.next()) count++;
                        if (count != 10) throw new RuntimeException("Iter " + i + ": Expected 10, got " + count);
                    }
                }
            }
            System.out.println("Test 4 complete");

            // Test 5: Mixed named and unnamed prepared statements with large data
            System.out.println("Test 5: Mixed named/unnamed prepared statements");
            try (Connection connection = dataSource.getConnection()) {
                for (int round = 0; round < 15; round++) {
                    // Unnamed (simple statement)
                    try (Statement stmt = connection.createStatement();
                         ResultSet rs = stmt.executeQuery("SELECT * FROM test_stress_describe WHERE id = 1")) {
                        if (!rs.next()) throw new RuntimeException("Round " + round + ": Unnamed no row");
                    }

                    // Named (prepared)
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe ORDER BY id");
                         ResultSet rs = pstmt.executeQuery()) {
                        int count = 0;
                        while (rs.next()) count++;
                        if (count != 10) throw new RuntimeException("Round " + round + ": Named expected 10, got " + count);
                    }

                    // Another unnamed with different query
                    try (Statement stmt = connection.createStatement();
                         ResultSet rs = stmt.executeQuery("SELECT col1, col2 FROM test_stress_describe WHERE id > 5")) {
                        int count = 0;
                        while (rs.next()) count++;
                        if (count != 5) throw new RuntimeException("Round " + round + ": Unnamed2 expected 5, got " + count);
                    }
                }
            }
            System.out.println("Test 5 complete");

            // Test 6: Concurrent connections with different prepared statement lifecycles
            System.out.println("Test 6: Concurrent connections different lifecycles");
            {
                AtomicInteger errors = new AtomicInteger(0);
                ExecutorService executor = Executors.newFixedThreadPool(10);
                List<Future<?>> futures = new ArrayList<>();

                for (int clientId = 0; clientId < 10; clientId++) {
                    final int id = clientId;
                    futures.add(executor.submit(() -> {
                        try (Connection conn = dataSource.getConnection()) {
                            if (id % 3 == 0) {
                                // Long-lived prepared statement
                                try (PreparedStatement pstmt = conn.prepareStatement(
                                        "SELECT * FROM test_stress_describe ORDER BY id")) {
                                    for (int i = 0; i < 20; i++) {
                                        int count = 0;
                                        try (ResultSet rs = pstmt.executeQuery()) {
                                            while (rs.next()) count++;
                                        }
                                        if (count != 10) {
                                            errors.incrementAndGet();
                                            System.err.println("Client " + id + " iter " + i + ": Expected 10, got " + count);
                                        }
                                    }
                                }
                            } else if (id % 3 == 1) {
                                // Short-lived prepared statements
                                for (int i = 0; i < 20; i++) {
                                    try (PreparedStatement pstmt = conn.prepareStatement(
                                            "SELECT * FROM test_stress_describe WHERE id > ?")) {
                                        pstmt.setInt(1, i % 5);
                                        int count = 0;
                                        try (ResultSet rs = pstmt.executeQuery()) {
                                            while (rs.next()) count++;
                                        }
                                        int expected = 10 - (i % 5);
                                        if (count != expected) {
                                            errors.incrementAndGet();
                                            System.err.println("Client " + id + " iter " + i + ": Expected " + expected + ", got " + count);
                                        }
                                    }
                                }
                            } else {
                                // Unprepared queries only
                                for (int i = 0; i < 20; i++) {
                                    try (Statement stmt = conn.createStatement();
                                         ResultSet rs = stmt.executeQuery(
                                             "SELECT * FROM test_stress_describe WHERE id > " + (i % 5) + " ORDER BY id")) {
                                        int count = 0;
                                        while (rs.next()) count++;
                                        int expected = 10 - (i % 5);
                                        if (count != expected) {
                                            errors.incrementAndGet();
                                            System.err.println("Client " + id + " iter " + i + ": Expected " + expected + ", got " + count);
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

                if (errors.get() > 0) throw new RuntimeException("Concurrent lifecycle test failed with " + errors.get() + " errors");
            }
            System.out.println("Test 6 complete");

            // Test 7: Prepare with very large parameter count
            System.out.println("Test 7: Large parameter count stress test");
            try (Connection connection = dataSource.getConnection()) {
                StringBuilder createSql = new StringBuilder("DROP TABLE IF EXISTS test_many_params; CREATE TABLE test_many_params(id serial primary key");
                for (int i = 1; i <= 20; i++) {
                    createSql.append(", p").append(i).append(" text");
                }
                createSql.append(")");

                try (Statement stmt = connection.createStatement()) {
                    stmt.execute(createSql.toString());
                }

                // Insert with many parameters
                StringBuilder insertSql = new StringBuilder("INSERT INTO test_many_params(");
                StringBuilder valuesSql = new StringBuilder(" VALUES(");
                for (int i = 1; i <= 20; i++) {
                    if (i > 1) {
                        insertSql.append(", ");
                        valuesSql.append(", ");
                    }
                    insertSql.append("p").append(i);
                    valuesSql.append("?");
                }
                insertSql.append(")").append(valuesSql).append(")");

                for (int row = 0; row < 10; row++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(insertSql.toString())) {
                        for (int i = 1; i <= 20; i++) {
                            pstmt.setString(i, repeat((char) ('A' + i), 200));
                        }
                        pstmt.executeUpdate();
                    }
                }

                // Select all
                try (PreparedStatement pstmt = connection.prepareStatement(
                        "SELECT * FROM test_many_params ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    while (rs.next()) {
                        ResultSetMetaData meta = rs.getMetaData();
                        if (meta.getColumnCount() != 21) throw new RuntimeException("Expected 21 columns, got " + meta.getColumnCount());
                        count++;
                    }
                    if (count != 10) throw new RuntimeException("Expected 10 rows, got " + count);
                }
            }
            System.out.println("Test 7 complete");

            // Test 8: Rapid connection cycling with prepared statements
            System.out.println("Test 8: Rapid connection cycling with prepared statements");
            for (int cycle = 0; cycle < 30; cycle++) {
                try (Connection connection = dataSource.getConnection();
                     PreparedStatement pstmt = connection.prepareStatement(
                         "SELECT * FROM test_stress_describe ORDER BY id");
                     ResultSet rs = pstmt.executeQuery()) {
                    int count = 0;
                    while (rs.next()) count++;
                    if (count != 10) throw new RuntimeException("Cycle " + cycle + ": Expected 10, got " + count);
                }
            }
            System.out.println("Test 8 complete");

            // Test 9: Batch execution simulation
            System.out.println("Test 9: Batch execution simulation");
            try (Connection connection = dataSource.getConnection()) {
                for (int batch = 0; batch < 10; batch++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe WHERE id > ? ORDER BY id")) {
                        for (int i = 0; i < 5; i++) {
                            pstmt.setInt(1, i);
                            int count = 0;
                            try (ResultSet rs = pstmt.executeQuery()) {
                                while (rs.next()) count++;
                            }
                            int expected = 10 - i;
                            if (count != expected) throw new RuntimeException("Batch " + batch + " cmd " + i + ": Expected " + expected + ", got " + count);
                        }
                    }
                }
            }
            System.out.println("Test 9 complete");

            // Test 10: Extended protocol stress
            System.out.println("Test 10: Extended protocol stress");
            try (Connection connection = dataSource.getConnection()) {
                for (int i = 0; i < 30; i++) {
                    try (PreparedStatement pstmt = connection.prepareStatement(
                            "SELECT * FROM test_stress_describe WHERE id > ? ORDER BY id")) {
                        pstmt.setInt(1, i % 5);

                        // First execution
                        int count1 = 0;
                        try (ResultSet rs = pstmt.executeQuery()) {
                            while (rs.next()) count1++;
                        }

                        // Second execution with different parameter
                        pstmt.setInt(1, (i + 2) % 5);
                        int count2 = 0;
                        try (ResultSet rs = pstmt.executeQuery()) {
                            while (rs.next()) count2++;
                        }

                        int expected1 = 10 - (i % 5);
                        int expected2 = 10 - ((i + 2) % 5);
                        if (count1 != expected1) throw new RuntimeException("Iter " + i + " exec1: Expected " + expected1 + ", got " + count1);
                        if (count2 != expected2) throw new RuntimeException("Iter " + i + " exec2: Expected " + expected2 + ", got " + count2);
                    }
                }
            }
            System.out.println("Test 10 complete");

            System.out.println("prepared_stress_describe complete");
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
