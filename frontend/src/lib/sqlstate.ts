// Human-readable PostgreSQL SQLSTATE labels. Covers the codes that operators
// see day-to-day plus the pg_doorman-side ones (53300 on checkout failure,
// 26000 on synthetic prepared-statement miss). Unknown codes fall back to
// their two-character class prefix.

const STATEMENTS: Record<string, string> = {
  // pg_doorman side
  "53300": "too_many_connections (pg_doorman checkout fail)",
  "26000": "invalid_sql_statement_name (synthetic miss)",
  "57P01": "admin_shutdown",
  "57P02": "crash_shutdown",
  "57P03": "cannot_connect_now",
  "58006": "internal_error (pg_doorman shutdown)",
  // PostgreSQL — common
  "08000": "connection_exception",
  "08003": "connection_does_not_exist",
  "08006": "connection_failure",
  "22000": "data_exception",
  "22001": "string_data_right_truncation",
  "22003": "numeric_value_out_of_range",
  "22P02": "invalid_text_representation",
  "23000": "integrity_constraint_violation",
  "23502": "not_null_violation",
  "23503": "foreign_key_violation",
  "23505": "unique_violation",
  "23514": "check_violation",
  "25000": "invalid_transaction_state",
  "25P02": "in_failed_sql_transaction",
  "28000": "invalid_authorization_specification",
  "28P01": "invalid_password",
  "40000": "transaction_rollback",
  "40001": "serialization_failure",
  "40P01": "deadlock_detected",
  "42000": "syntax_error_or_access_rule_violation",
  "42501": "insufficient_privilege",
  "42601": "syntax_error",
  "42703": "undefined_column",
  "42P01": "undefined_table",
  "53000": "insufficient_resources",
  "53100": "disk_full",
  "53200": "out_of_memory",
  "55P03": "lock_not_available",
  "57000": "operator_intervention",
  "57014": "query_canceled",
  "XX000": "internal_error",
};

const CLASSES: Record<string, string> = {
  "00": "Successful Completion",
  "01": "Warning",
  "02": "No Data",
  "03": "SQL Statement Not Yet Complete",
  "08": "Connection Exception",
  "09": "Triggered Action Exception",
  "0A": "Feature Not Supported",
  "0B": "Invalid Transaction Initiation",
  "0F": "Locator Exception",
  "0L": "Invalid Grantor",
  "0P": "Invalid Role Specification",
  "0Z": "Diagnostics Exception",
  "20": "Case Not Found",
  "21": "Cardinality Violation",
  "22": "Data Exception",
  "23": "Integrity Constraint Violation",
  "24": "Invalid Cursor State",
  "25": "Invalid Transaction State",
  "26": "Invalid SQL Statement Name",
  "27": "Triggered Data Change Violation",
  "28": "Invalid Authorization Specification",
  "2B": "Dependent Privilege Descriptors Still Exist",
  "2D": "Invalid Transaction Termination",
  "2F": "SQL Routine Exception",
  "34": "Invalid Cursor Name",
  "38": "External Routine Exception",
  "39": "External Routine Invocation Exception",
  "3B": "Savepoint Exception",
  "3D": "Invalid Catalog Name",
  "3F": "Invalid Schema Name",
  "40": "Transaction Rollback",
  "42": "Syntax or Access Rule Violation",
  "44": "WITH CHECK OPTION Violation",
  "53": "Insufficient Resources",
  "54": "Program Limit Exceeded",
  "55": "Object Not in Prerequisite State",
  "57": "Operator Intervention",
  "58": "System Error",
  "72": "Snapshot Too Old",
  F0: "Configuration File Error",
  HV: "Foreign Data Wrapper Error",
  P0: "PL/pgSQL Error",
  XX: "Internal Error",
};

export function describeSqlstate(code: string): string {
  const exact = STATEMENTS[code];
  if (exact) return exact;
  const klass = code.slice(0, 2);
  const cls = CLASSES[klass];
  if (cls) return `class ${klass}: ${cls}`;
  return "unknown SQLSTATE";
}
