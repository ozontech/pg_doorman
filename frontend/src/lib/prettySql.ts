// Lightweight SQL pretty-printer for the Prepared expand row. We do not
// pull a real SQL parser into the bundle — a 50-line keyword-driven
// reformatter is enough for the prepared statements pg_doorman caches
// (they're issued by ORMs and pgbench-style scripts, not hand-written
// reports). Behaviour:
//
//   - Trim and collapse runs of whitespace down to a single space.
//   - Insert a newline before every "major clause" keyword
//     (SELECT, FROM, WHERE, JOIN variants, AND, OR, GROUP BY, ORDER BY,
//      LIMIT, OFFSET, RETURNING, VALUES, ON CONFLICT, UNION...).
//   - Continuation lines indent two spaces so the eye picks up the
//     hierarchy.
//   - Keywords stay in whatever case the SQL ships with — uppercasing
//     would mangle quoted identifiers.
//
// The split is regex-only and case-insensitive. Quoted strings, identifiers,
// and column names are left alone because the regex requires whitespace
// boundaries and the SQL we receive is already paramaterised.

const MAJOR_CLAUSES = [
  "SELECT",
  "INSERT INTO",
  "UPDATE",
  "DELETE FROM",
  "FROM",
  "WHERE",
  "GROUP BY",
  "ORDER BY",
  "HAVING",
  "LIMIT",
  "OFFSET",
  "RETURNING",
  "VALUES",
  "SET",
  "ON CONFLICT",
  "UNION",
  "UNION ALL",
  "INTERSECT",
  "EXCEPT",
  "INNER JOIN",
  "LEFT JOIN",
  "LEFT OUTER JOIN",
  "RIGHT JOIN",
  "RIGHT OUTER JOIN",
  "FULL JOIN",
  "FULL OUTER JOIN",
  "CROSS JOIN",
  "JOIN",
  "WITH",
];

const MINOR_CONNECTORS = ["AND", "OR"];

export function prettySql(raw: string): string {
  if (!raw) return "";
  // Collapse whitespace runs, drop leading/trailing blanks.
  const collapsed = raw.replace(/\s+/g, " ").trim();
  if (collapsed.length === 0) return "";

  let result = collapsed;

  // Major clauses → newline + zero indent.
  for (const kw of MAJOR_CLAUSES) {
    const re = new RegExp(`(\\s|^)${kw.replace(/ /g, "\\s+")}\\b`, "gi");
    result = result.replace(re, (_match, lead) => {
      const prefix = lead === "" ? "" : "\n";
      return `${prefix}${kw}`;
    });
  }

  // Minor connectors (AND / OR) inside WHERE / ON → newline + 2-space indent.
  for (const kw of MINOR_CONNECTORS) {
    const re = new RegExp(`\\s${kw}\\s`, "gi");
    result = result.replace(re, `\n  ${kw} `);
  }

  // Trim leading newline if the very first token was a clause keyword.
  return result.replace(/^\n/, "");
}
