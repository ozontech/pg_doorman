<?php

$dsn = getenv('DATABASE_URL');
if (!$dsn) {
    fwrite(STDERR, "DATABASE_URL environment variable is not set\n");
    exit(1);
}

$pdo = new PDO($dsn);
$pdo->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);

// Get backend PID before error
$pid_before = $pdo->query('SELECT pg_backend_pid()')->fetchColumn();
echo "Backend PID before error: $pid_before\n";

// Trigger SQL error
try {
    $pdo->query('SELECT 1/0');
} catch (PDOException $e) {
    echo "Got expected error: " . $e->getMessage() . "\n";
}

// Get backend PID after error — must be the same
$pid_after = $pdo->query('SELECT pg_backend_pid()')->fetchColumn();
echo "Backend PID after error: $pid_after\n";

if ($pid_before !== $pid_after) {
    fwrite(STDERR, "FAIL: backend PID changed from $pid_before to $pid_after\n");
    exit(1);
}

echo "session_mode_error complete\n";
