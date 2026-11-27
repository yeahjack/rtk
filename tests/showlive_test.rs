//! Contract tests for the read-only live hook view, using isolated history.
mod common;

use rusqlite::Connection;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(8);

fn command(path: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("RTK_DB_PATH", path)
        .env_remove("RTK_HOOK_AUDIT")
        .env("RTK_AUDIT_DIR", path.parent().expect("fixture parent"));
    cmd
}

fn lines(reader: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}

struct Watcher {
    child: Child,
    out: Receiver<String>,
}

impl Watcher {
    fn start(path: &Path) -> Self {
        let mut child = command(path)
            .arg("showlive")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start rtk showlive");
        let out = lines(child.stdout.take().expect("stdout"));
        let status = lines(child.stderr.take().expect("stderr"));
        let this = Self { child, out };
        assert!(
            status
                .recv_timeout(WAIT)
                .expect("follower ready")
                .contains("Following new recorded")
        );
        this
    }

    fn next(&self) -> String {
        self.out.recv_timeout(WAIT).expect("live event")
    }

    fn quiet(&self) {
        assert_eq!(
            self.out.recv_timeout(Duration::from_millis(450)),
            Err(RecvTimeoutError::Timeout)
        );
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn claude(path: &Path, tool_id: &str, line: &str) {
    let mut child = command(path)
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start Claude hook");
    serde_json::to_writer(
        child.stdin.take().expect("hook stdin"),
        &serde_json::json!({
            "session_id": "session-marker",
            "tool_use_id": tool_id,
            "cwd": "/project-marker",
            "tool_name": "Bash",
            "hook_event_name": "PreToolUse",
            "tool_input": { "command": line },
        }),
    )
    .expect("write payload");
    let output = child.wait_with_output().expect("hook output");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn first_run_does_not_create_db_and_actual_claude_events_arrive() {
    let dir = common::tempdir();
    let path = dir.path().join("not-created").join("history.db");
    let watcher = Watcher::start(&path);
    assert!(!path.parent().unwrap().exists());
    claude(&path, "first-tool-marker", "git status -- file-marker");
    let record = watcher.next();
    assert!(
        record.ends_with("  rtk git  rewrite proposed (ask)")
            || record.ends_with("  rtk git  rewrite proposed (allow)"),
        "{record}"
    );
    assert!(!record.contains("marker"));
    claude(&path, "second-tool-marker", "unhandled-marker abc");
    assert!(watcher.next().ends_with("  Other  not rewritten (defer)"));
    watcher.quiet();
    let conn = Connection::open(&path).expect("read fixture");
    let logged: (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM hook_decisions), (SELECT COUNT(*) FROM commands)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("row counts");
    assert_eq!(logged, (2, 0));
    assert!(!path.parent().unwrap().join("hook-audit.log").exists());
}

fn insert(conn: &Connection, timestamp: &str, outcome: &str) {
    conn.execute(
        "INSERT INTO hook_decisions (timestamp, session_id, tool_use_id, raw_cmd, decision, rtk_version)
         VALUES (?1, 'hidden-session', 'hidden-tool-id', 'hidden-program secret', ?2, '0.0.0')",
        [timestamp, outcome],
    )
    .expect("seed decision");
}

#[test]
fn ignores_backlog_then_follows_committed_batches_in_id_order() {
    let dir = common::tempdir();
    let path = dir.path().join("history.db");
    claude(&path, "backlogged", "git status");
    let watcher = Watcher::start(&path);
    watcher.quiet();

    let mut conn = Connection::open(&path).expect("open writer");
    let tx = conn.transaction().expect("writer transaction");
    // Same timestamp, unknown decision and more than a full query page:
    // events should be in ID order and the invalid entry must not pin a cursor.
    insert(&tx, "2026-11-20T00:00:00Z", "unknown-marker");
    for index in 0..135 {
        insert(
            &tx,
            "2026-11-20T00:00:00Z",
            if index % 2 == 0 { "allow" } else { "deny" },
        );
    }
    watcher.quiet();
    tx.commit().expect("commit writer");
    for index in 0..135 {
        let expected = if index % 2 == 0 {
            "00:00:00Z  Other  rewrite proposed (allow)"
        } else {
            "00:00:00Z  Other  not rewritten (deny)"
        };
        assert_eq!(watcher.next(), expected);
    }
    watcher.quiet();
}

#[test]
fn empty_legacy_db_is_not_migrated() {
    let dir = common::tempdir();
    let path = dir.path().join("history.db");
    Connection::open(&path)
        .expect("open untagged DB")
        .execute_batch("CREATE TABLE commands (id INTEGER PRIMARY KEY);")
        .expect("seed legacy DB");
    let before = std::fs::read(&path).expect("read fixture");
    let watcher = Watcher::start(&path);
    watcher.quiet();
    drop(watcher);
    assert_eq!(std::fs::read(&path).expect("read unchanged DB"), before);
}

#[test]
fn rejects_other_rtk_database_without_changes() {
    let dir = common::tempdir();
    let path = dir.path().join("recall.db");
    Connection::open(&path)
        .expect("open foreign DB")
        .execute_batch("PRAGMA application_id = 0x52544B02;")
        .expect("tag foreign DB");
    let before = std::fs::read(&path).expect("read fixture");
    let output = command(&path).arg("showlive").output().expect("showlive");
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read(&path).expect("read unchanged DB"), before);
}

#[cfg(unix)]
#[test]
fn ctrl_c_interrupts_an_idle_follower() {
    use std::os::unix::process::ExitStatusExt;
    let dir = common::tempdir();
    let path = dir.path().join("history.db");
    let mut watcher = Watcher::start(&path);
    let signal = common::native_command("kill")
        .args(["-INT", &watcher.child.id().to_string()])
        .status()
        .expect("signal follower");
    assert!(signal.success());
    let start = std::time::Instant::now();
    while start.elapsed() < WAIT {
        if let Some(status) = watcher.child.try_wait().expect("wait for follower") {
            assert_eq!(status.signal(), Some(libc::SIGINT));
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("follower did not stop after Ctrl-C");
}

#[test]
fn retries_a_transient_exclusive_writer_lock() {
    let dir = common::tempdir();
    let path = dir.path().join("history.db");
    claude(&path, "seed", "git status");
    let conn = Connection::open(&path).expect("writer");
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
        .expect("exercise rollback journal locking");
    assert_eq!(mode, "delete");
    let watcher = Watcher::start(&path);
    conn.execute_batch("BEGIN EXCLUSIVE;")
        .expect("hold lock over at least one poll");
    insert(&conn, "2026-11-20T00:00:00Z", "defer");
    watcher.quiet();
    conn.execute_batch("COMMIT;").expect("commit writer");
    assert_eq!(watcher.next(), "00:00:00Z  Other  not rewritten (defer)");
}
