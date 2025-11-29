import sys
import time
import threading
import subprocess
from textwrap import dedent


def _run_cancel_scenario():
    """
    Run a long query and cancel it using libpq (psycopg2) connection.cancel().
    This function is intended to be executed in a subprocess so that any
    libpq messages written directly to stderr (C-level) are captured by the
    parent process for assertions.
    """
    script = dedent(
        r'''
        import time
        import threading
        import psycopg2

        conn = psycopg2.connect(
        host="localhost",
        port=6433,
        dbname="example_db",
        user="example_user_1",
        password="test",
        sslmode="disable",
        )

        cur = conn.cursor()

        def run_query():
            try:
                # Long running query to ensure it's in ACTIVE state.
                cur.execute("select 1, pg_sleep(10)")
            except Exception:
                # We expect a cancellation error here; swallow it.
                pass

        t = threading.Thread(target=run_query)
        t.start()
        # Give the query time to start and be ACTIVE on the server.
        time.sleep(1.0)

        # Issue a cancel request using libpq mechanism.
        # If pg_doorman is configured correctly for cancel sockets,
        # libpq should not emit warnings about failed cancel connection.
        conn.cancel()

        t.join()
        cur.close()
        conn.close()
        print("done test")
        '''
    )

    # Run the scenario in a fresh Python interpreter to capture real stderr.
    completed = subprocess.run(
        [sys.executable, "-c", script],
        capture_output=True,
        text=True,
        check=False,
    )
    return completed.returncode, completed.stdout, completed.stderr


def test_cancel_does_not_emit_libpq_failure_message():
    code, out, err = _run_cancel_scenario()
    print("Cancel scenario: subprocess should exit with code 0 after handling cancellation.")
    print("Verifying clean cancellation path (no libpq stderr noise)...")
    assert code == 0, (
        f"Expected subprocess exit code 0 (cancellation handled). Got {code}.\n"
        f"stdout: {out}\n"
        f"stderr: {err}"
    )
    undesired = "query cancellation failed: cancellation failed: connection to server"
    print("Asserting the noisy libpq PQcancel failure message is absent from stderr...")
    assert undesired not in err, (
        "Unexpected libpq stderr noise detected (PQcancel failure).\n"
        f"Looked for: '{undesired}'\n"
        f"stderr: {err}"
    )
    desired = "done test"
    assert desired in out, (
        f"Expected completion anchor '{desired}' in stdout to confirm script finished.\n"
        f"stdout: {out}"
    )
