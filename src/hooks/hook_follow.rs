//! Read-only follow view of the existing Claude Code hook decision history.
use crate::core::tracking::{HookOutcome, get_db_path, open_hook_decision_history};
use crate::discover::registry::{Classification, classify_command};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, ErrorCode, params};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

const BATCH: usize = 128;
const POLL: Duration = Duration::from_millis(250);

struct Decision {
    id: i64,
    timestamp: String,
    outcome: String,
    command_prefix: String,
}

pub fn run() -> Result<()> {
    let path = get_db_path()?;
    let mut cursor = loop {
        match read_history(&path, |conn| {
            Ok(conn.query_row(
                "SELECT COALESCE(MAX(id), 0) FROM hook_decisions",
                [],
                |row| row.get(0),
            )?)
        }) {
            Ok(last) => break last.unwrap_or(0),
            Err(error) if is_busy(&error) => std::thread::sleep(POLL),
            Err(error) => return Err(error),
        }
    };
    eprintln!("Following new recorded Claude Code hook decisions. Press Ctrl-C to stop.");
    loop {
        match read_history(&path, |conn| decisions_after(conn, cursor)) {
            Ok(Some(rows)) if !rows.is_empty() => {
                let caught_up = rows.len() < BATCH;
                if let Err(error) = write_decisions(&rows, &mut io::stdout().lock()) {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(error).context("Failed to write live hook decisions");
                }
                cursor = rows.last().map_or(cursor, |row| row.id);
                if !caught_up {
                    continue;
                }
            }
            Ok(_) => (),
            Err(error) if is_busy(&error) => (),
            Err(error) => return Err(error),
        }
        std::thread::sleep(POLL);
    }
}

fn read_history<T>(path: &Path, query: impl FnOnce(&Connection) -> Result<T>) -> Result<Option<T>> {
    let Some(conn) = open_hook_decision_history(path)? else {
        return Ok(None);
    };
    let has_decisions = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'hook_decisions')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_decisions {
        return Ok(None);
    }
    query(&conn).map(Some)
}

fn is_busy(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(error, _))
            if matches!(error.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn decisions_after(conn: &Connection, cursor: i64) -> Result<Vec<Decision>> {
    // Use the integer key rather than timestamp: events can share a clock tick.
    // Only the bounded command prefix is needed to classify a display category.
    let mut stmt = conn.prepare(
        "SELECT id, timestamp, decision, substr(raw_cmd, 1, 512)
         FROM hook_decisions WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![cursor, BATCH as i64], |row| {
            Ok(Decision {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                outcome: row.get(2)?,
                command_prefix: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn write_decisions(rows: &[Decision], out: &mut impl Write) -> io::Result<()> {
    for row in rows {
        let (Ok(timestamp), Ok(outcome)) = (
            DateTime::parse_from_rfc3339(&row.timestamp),
            row.outcome.parse::<HookOutcome>(),
        ) else {
            continue;
        };
        // Unsupported command names and words are user input; never print them.
        let label = match classify_command(&row.command_prefix) {
            Classification::Supported { rtk_equivalent, .. } => rtk_equivalent,
            _ => "Other",
        };
        let decision = match outcome {
            HookOutcome::Allow => "rewrite proposed (allow)",
            HookOutcome::Ask => "rewrite proposed (ask)",
            HookOutcome::Deny => "not rewritten (deny)",
            HookOutcome::Defer => "not rewritten (defer)",
        };
        writeln!(
            out,
            "{}  {label}  {decision}",
            timestamp.with_timezone(&Utc).format("%H:%M:%SZ")
        )?;
    }
    out.flush()
}
