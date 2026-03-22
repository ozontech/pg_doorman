<?php

$dsn = getenv('DATABASE_URL');
if (!$dsn) {
    fwrite(STDERR, "DATABASE_URL environment variable is not set\n");
    exit(1);
}

$pdo = new PDO($dsn);
$pdo->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);

$stmt = $pdo->query('SELECT 1 AS result');
$row = $stmt->fetch(PDO::FETCH_ASSOC);

if ($row['result'] != '1') {
    fwrite(STDERR, "Expected '1', got '{$row['result']}'\n");
    exit(1);
}

echo "simple_select complete\n";
