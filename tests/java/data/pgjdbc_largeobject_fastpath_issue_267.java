import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;
import org.postgresql.PGConnection;
import org.postgresql.largeobject.LargeObject;
import org.postgresql.largeobject.LargeObjectManager;

import java.nio.charset.StandardCharsets;
import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.Statement;
import java.util.Arrays;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public class Main {
    private static final int OPERATION_TIMEOUT_SECONDS = 15;
    private static final int PAYLOAD_SIZE = 2 * 1024 * 1024 + 17;

    public static void main(String[] args) {
        ExecutorService executor = Executors.newSingleThreadExecutor();
        Future<?> task = executor.submit(() -> {
            runLargeObjectRoundTrip();
            return null;
        });

        try {
            task.get(OPERATION_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            System.out.println("issue_267_pgjdbc_lob complete");
        } catch (TimeoutException e) {
            System.err.println("LargeObject API call timed out");
            System.exit(2);
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
            e.printStackTrace();
            System.exit(1);
        } finally {
            executor.shutdownNow();
        }
    }

    private static void runLargeObjectRoundTrip() throws Exception {
        String databaseUrl = System.getenv("DATABASE_URL");
        if (databaseUrl == null || databaseUrl.isEmpty()) {
            databaseUrl = "jdbc:postgresql://127.0.0.1:6433/example_db?user=example_user_1&password=test";
        }

        HikariConfig config = new HikariConfig();
        config.setJdbcUrl(databaseUrl);
        config.setMaximumPoolSize(1);
        config.setMinimumIdle(0);
        config.setConnectionTimeout(5000);

        try (HikariDataSource dataSource = new HikariDataSource(config);
             Connection connection = dataSource.getConnection()) {
            connection.setAutoCommit(false);

            try (Statement statement = connection.createStatement()) {
                statement.execute("DROP TABLE IF EXISTS issue_267_documents");
                statement.execute("CREATE TABLE issue_267_documents(id integer primary key, payload oid)");
            }

            PGConnection pgConnection = connection.unwrap(PGConnection.class);
            LargeObjectManager manager = pgConnection.getLargeObjectAPI();

            long oid = manager.createLO(LargeObjectManager.READ | LargeObjectManager.WRITE);
            byte[] payload = buildPayload();

            try {
                LargeObject writer = manager.open(oid, LargeObjectManager.WRITE);
                try {
                    writer.write(payload);
                } finally {
                    writer.close();
                }

                try (PreparedStatement insert = connection.prepareStatement(
                    "INSERT INTO issue_267_documents(id, payload) VALUES (?, ?)"
                )) {
                    insert.setInt(1, 1);
                    insert.setLong(2, oid);
                    insert.executeUpdate();
                }

                long storedOid;
                try (PreparedStatement select = connection.prepareStatement(
                    "SELECT payload FROM issue_267_documents WHERE id = ?"
                )) {
                    select.setInt(1, 1);
                    try (ResultSet resultSet = select.executeQuery()) {
                        if (!resultSet.next()) {
                            throw new IllegalStateException("large object row was not stored");
                        }
                        storedOid = resultSet.getLong(1);
                    }
                }

                LargeObject reader = manager.open(storedOid, LargeObjectManager.READ);
                byte[] actual;
                try {
                    actual = reader.read(payload.length);
                } finally {
                    reader.close();
                }

                if (!Arrays.equals(payload, actual)) {
                    throw new IllegalStateException("large object payload mismatch");
                }

                manager.delete(oid);
                connection.commit();
            } catch (Exception e) {
                connection.rollback();
                throw e;
            }
        }
    }

    private static byte[] buildPayload() {
        byte[] payload = new byte[PAYLOAD_SIZE];
        byte[] marker = "issue-267-large-object-payload".getBytes(StandardCharsets.UTF_8);
        for (int i = 0; i < payload.length; i++) {
            payload[i] = (byte) (marker[i % marker.length] ^ (i & 0x7f));
        }
        return payload;
    }
}
