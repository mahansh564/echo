pub mod models;

use anyhow::{anyhow, Result};
use models::{AgentRow, ManagedSession, RuntimeIssue, SessionAlert, SessionEvent, Task};
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool};
use std::str::FromStr;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

const AGENT_SELECT_COLUMNS: &str = "id, name, state, provider, display_order, attention_state, task_id, active_session_id, last_snippet, last_input_required_at, updated_at";
const MANAGED_SESSION_SELECT_COLUMNS: &str = "id, provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, needs_input, input_reason, last_activity_at, transport, attach_count, failure_reason, metadata_json, created_at, updated_at";
const SESSION_ALERT_SELECT_COLUMNS: &str = "id, session_id, agent_id, severity, reason, message, message_enriched, message_enrichment_status, message_enriched_at, message_enrichment_error, requires_ack, acknowledged_at, snoozed_until, escalated_at, escalation_count, resolved_at, created_at, updated_at";
const RUNTIME_ISSUE_SELECT_COLUMNS: &str = "kind, source, raw_message, enriched_message, enrichment_status, enrichment_error, first_seen_at, last_seen_at, seen_count, dismissed_until, resolved_at";

#[derive(Debug, Clone, Default)]
pub struct AlertEnrichmentInput {
    pub message_enriched: Option<String>,
    pub message_enrichment_status: Option<String>,
    pub message_enrichment_error: Option<String>,
}

impl Db {
    async fn refresh_agent_attention_state_with_conn(
        conn: &mut PoolConnection<Sqlite>,
        agent_id: i64,
    ) -> Result<()> {
        sqlx::query(
            "WITH flags AS (
                SELECT
                    EXISTS(
                        SELECT 1
                        FROM session_alerts sa
                        WHERE sa.agent_id = ?
                          AND sa.resolved_at IS NULL
                          AND (sa.snoozed_until IS NULL OR sa.snoozed_until <= CURRENT_TIMESTAMP)
                          AND LOWER(sa.severity) = 'critical'
                    ) AS has_critical_alert,
                    EXISTS(
                        SELECT 1
                        FROM session_alerts sa
                        WHERE sa.agent_id = ?
                          AND sa.resolved_at IS NULL
                          AND (sa.snoozed_until IS NULL OR sa.snoozed_until <= CURRENT_TIMESTAMP)
                    ) AS has_open_alert,
                    EXISTS(
                        SELECT 1
                        FROM managed_sessions ms
                        WHERE ms.agent_id = ?
                          AND ms.needs_input = 1
                          AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
                    ) AS has_input_needed_session
            )
            UPDATE agents
            SET attention_state = CASE
                    WHEN (SELECT has_critical_alert FROM flags) = 1 THEN 'blocked'
                    WHEN (SELECT has_open_alert FROM flags) = 1
                      OR (SELECT has_input_needed_session FROM flags) = 1 THEN 'needs_input'
                    ELSE 'ok'
                END,
                last_input_required_at = CASE
                    WHEN (SELECT has_open_alert FROM flags) = 1
                      OR (SELECT has_input_needed_session FROM flags) = 1
                    THEN COALESCE(last_input_required_at, CURRENT_TIMESTAMP)
                    ELSE last_input_required_at
                END,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?",
        )
        .bind(agent_id)
        .bind(agent_id)
        .bind(agent_id)
        .bind(agent_id)
        .execute(&mut **conn)
        .await?;
        Ok(())
    }

    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = if database_url.starts_with("sqlite:") {
            let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);
            SqlitePoolOptions::new()
                .max_connections(5)
                .connect_with(options)
                .await?
        } else {
            SqlitePoolOptions::new()
                .max_connections(5)
                .connect(database_url)
                .await?
        };
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn create_task(&self, title: &str, state: Option<&str>) -> Result<Task> {
        let mut conn = self.pool.acquire().await?;
        let state_value = state.unwrap_or("todo");
        sqlx::query("INSERT INTO tasks (title, state) VALUES (?, ?)")
            .bind(title)
            .bind(state_value)
            .execute(&mut *conn)
            .await?;

        let task = sqlx::query_as::<_, Task>(
            "SELECT id, title, state, updated_at FROM tasks WHERE id = last_insert_rowid()",
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(task)
    }

    pub async fn update_task(
        &self,
        id: i64,
        title: Option<&str>,
        state: Option<&str>,
    ) -> Result<Task> {
        sqlx::query(
            "UPDATE tasks SET title = COALESCE(?, title), state = COALESCE(?, state), updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(title)
        .bind(state)
        .bind(id)
        .execute(&self.pool)
        .await?;
        self.get_task(id).await
    }

    pub async fn delete_task(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn move_task_state(&self, id: i64, state: &str) -> Result<Task> {
        self.update_task(id, None, Some(state)).await
    }

    pub async fn get_task(&self, id: i64) -> Result<Task> {
        let task = sqlx::query_as::<_, Task>(
            "SELECT id, title, state, updated_at FROM tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(task)
    }

    pub async fn create_agent(
        &self,
        name: &str,
        provider: Option<&str>,
        state: Option<&str>,
        task_id: Option<i64>,
    ) -> Result<models::Agent> {
        let mut conn = self.pool.acquire().await?;
        let state_value = state.unwrap_or("idle");
        let provider_value = provider.unwrap_or("opencode");
        sqlx::query("INSERT INTO agents (name, provider, state, task_id) VALUES (?, ?, ?, ?)")
            .bind(name)
            .bind(provider_value)
            .bind(state_value)
            .bind(task_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE agents SET display_order = id WHERE id = last_insert_rowid()")
            .execute(&mut *conn)
            .await?;

        let agent = sqlx::query_as::<_, models::Agent>(&format!(
            "SELECT {} FROM agents WHERE id = last_insert_rowid()",
            AGENT_SELECT_COLUMNS
        ))
        .fetch_one(&mut *conn)
        .await?;

        Ok(agent)
    }

    pub async fn assign_agent_to_task(
        &self,
        agent_id: i64,
        task_id: Option<i64>,
    ) -> Result<models::Agent> {
        sqlx::query("UPDATE agents SET task_id = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(task_id)
            .bind(agent_id)
            .execute(&self.pool)
            .await?;

        let agent = sqlx::query_as::<_, models::Agent>(&format!(
            "SELECT {} FROM agents WHERE id = ?",
            AGENT_SELECT_COLUMNS
        ))
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(agent)
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as::<_, Task>(
            "SELECT id, title, state, updated_at FROM tasks ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(tasks)
    }

    pub async fn list_agents(&self) -> Result<Vec<models::Agent>> {
        let agents = sqlx::query_as::<_, models::Agent>(&format!(
            "SELECT {} FROM agents ORDER BY display_order ASC, updated_at DESC",
            AGENT_SELECT_COLUMNS
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(agents)
    }

    pub async fn get_agent(&self, agent_id: i64) -> Result<models::Agent> {
        let agent = sqlx::query_as::<_, models::Agent>(&format!(
            "SELECT {} FROM agents WHERE id = ?",
            AGENT_SELECT_COLUMNS
        ))
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(agent)
    }

    pub async fn list_agent_rows(&self, limit: Option<i64>) -> Result<Vec<AgentRow>> {
        let max = limit.unwrap_or(100);
        let rows = sqlx::query_as::<_, AgentRow>(
            "WITH latest_session AS (
                SELECT
                    ms.id,
                    ms.agent_id,
                    ms.status,
                    ms.needs_input,
                    ms.input_reason,
                    ms.last_activity_at,
                    ms.last_heartbeat_at,
                    ms.updated_at,
                    ROW_NUMBER() OVER (
                        PARTITION BY ms.agent_id
                        ORDER BY ms.updated_at DESC, ms.id DESC
                    ) AS rn
                FROM managed_sessions ms
                WHERE ms.agent_id IS NOT NULL
                  AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
            ),
            open_alert_counts AS (
                SELECT sa.agent_id, COUNT(*) AS unresolved_alert_count
                FROM session_alerts sa
                WHERE sa.resolved_at IS NULL
                  AND (sa.snoozed_until IS NULL OR sa.snoozed_until <= CURRENT_TIMESTAMP)
                GROUP BY sa.agent_id
            )
            SELECT
                a.id AS agent_id,
                a.name AS agent_name,
                a.state AS agent_state,
                a.provider,
                a.display_order,
                a.attention_state,
                a.task_id,
                t.title AS task_title,
                COALESCE(a.active_session_id, ls.id) AS active_session_id,
                ls.status AS active_session_status,
                ls.needs_input AS active_session_needs_input,
                ls.input_reason AS active_session_input_reason,
                COALESCE(ls.last_activity_at, ls.last_heartbeat_at, ls.updated_at) AS last_activity_at,
                a.last_snippet,
                COALESCE(oac.unresolved_alert_count, 0) AS unresolved_alert_count,
                a.updated_at
            FROM agents a
            LEFT JOIN tasks t ON t.id = a.task_id
            LEFT JOIN latest_session ls ON ls.agent_id = a.id AND ls.rn = 1
            LEFT JOIN open_alert_counts oac ON oac.agent_id = a.id
            ORDER BY a.display_order ASC, a.updated_at DESC
            LIMIT ?",
        )
        .bind(max)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn update_agent_snippet(&self, agent_id: i64, snippet: &str) -> Result<()> {
        sqlx::query(
            "UPDATE agents SET last_snippet = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(snippet)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_managed_session(
        &self,
        provider: &str,
        launch_command: &str,
        launch_args_json: &str,
        cwd: Option<&str>,
        agent_id: Option<i64>,
        task_id: Option<i64>,
        metadata_json: Option<&str>,
    ) -> Result<ManagedSession> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "INSERT INTO managed_sessions (provider, status, launch_command, launch_args_json, cwd, agent_id, task_id, metadata_json) VALUES (?, 'waking', ?, ?, ?, ?, ?, ?)",
        )
        .bind(provider)
        .bind(launch_command)
        .bind(launch_args_json)
        .bind(cwd)
        .bind(agent_id)
        .bind(task_id)
        .bind(metadata_json)
        .execute(&mut *conn)
        .await?;

        let session = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = last_insert_rowid()",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = agent_id {
            sqlx::query(
                "UPDATE agents SET active_session_id = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(session.id)
            .bind(agent_id)
            .execute(&mut *conn)
            .await?;
        }

        Ok(session)
    }

    pub async fn update_session_status(
        &self,
        session_id: i64,
        status: &str,
        failure_reason: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE managed_sessions SET status = ?, failure_reason = ?, started_at = CASE WHEN ? = 'active' AND started_at IS NULL THEN CURRENT_TIMESTAMP ELSE started_at END, ended_at = CASE WHEN ? IN ('ended', 'failed') THEN CURRENT_TIMESTAMP ELSE ended_at END, attach_count = CASE WHEN ? IN ('ended', 'failed') THEN 0 ELSE attach_count END, last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(status)
        .bind(failure_reason)
        .bind(status)
        .bind(status)
        .bind(status)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        if matches!(status, "ended" | "failed") {
            sqlx::query(
                "UPDATE agents
                 SET active_session_id = (
                     SELECT ms.id
                     FROM managed_sessions ms
                     WHERE ms.agent_id = agents.id
                       AND ms.id <> ?
                       AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
                     ORDER BY COALESCE(ms.last_activity_at, ms.last_heartbeat_at, ms.updated_at) DESC, ms.id DESC
                     LIMIT 1
                 ),
                 updated_at = CURRENT_TIMESTAMP
                 WHERE active_session_id = ?",
            )
            .bind(session_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn end_session_if_open(&self, session_id: i64, reason: Option<&str>) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE managed_sessions
             SET status = 'ended',
                 failure_reason = ?,
                 ended_at = CURRENT_TIMESTAMP,
                 needs_input = 0,
                 input_reason = NULL,
                 attach_count = 0,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?
               AND status IN ('waking', 'active', 'stalled', 'needs_input')",
        )
        .bind(reason)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() > 0 {
            sqlx::query(
                "UPDATE agents
                 SET active_session_id = (
                     SELECT ms.id
                     FROM managed_sessions ms
                     WHERE ms.agent_id = agents.id
                       AND ms.id <> ?
                       AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
                     ORDER BY COALESCE(ms.last_activity_at, ms.last_heartbeat_at, ms.updated_at) DESC, ms.id DESC
                     LIMIT 1
                 ),
                 updated_at = CURRENT_TIMESTAMP
                 WHERE active_session_id = ?",
            )
                .bind(session_id)
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(true);
        }

        tx.commit().await?;
        Ok(false)
    }

    pub async fn mark_session_stalled_if_not_needs_input(&self, session_id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE managed_sessions
             SET status = 'stalled',
                 failure_reason = NULL,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?
               AND needs_input = 0
               AND status IN ('waking', 'active', 'stalled')",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_session_heartbeat(&self, session_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET last_heartbeat_at = CURRENT_TIMESTAMP, last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_session_needs_input(
        &self,
        session_id: i64,
        reason: &str,
        message: &str,
    ) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE managed_sessions
             SET status = 'needs_input',
                 needs_input = 1,
                 input_reason = ?,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(reason)
        .bind(session_id)
        .execute(&mut *conn)
        .await?;

        let row = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = row.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }

        sqlx::query(
            "INSERT INTO session_events (session_id, event_type, message, payload_json)
             VALUES (?, 'input_required', ?, ?)",
        )
        .bind(session_id)
        .bind(Some("session requires input"))
        .bind(Some(
            serde_json::json!({ "reason": reason, "message": message }).to_string(),
        ))
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn clear_session_needs_input(&self, session_id: i64) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE managed_sessions
             SET status = CASE WHEN status = 'needs_input' THEN 'active' ELSE status END,
                 needs_input = 0,
                 input_reason = NULL,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(session_id)
        .execute(&mut *conn)
        .await?;

        let row = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = row.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }
        Ok(())
    }

    pub async fn attach_session_context(
        &self,
        session_id: i64,
        agent_id: Option<i64>,
        task_id: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET agent_id = ?, task_id = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(agent_id)
        .bind(task_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn attach_terminal_session(&self, session_id: i64) -> Result<ManagedSession> {
        let mut conn = self.pool.acquire().await?;
        let current = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
        if matches!(current.status.as_str(), "ended" | "failed") {
            return Err(anyhow!("cannot attach to session in {}", current.status));
        }

        sqlx::query(
            "UPDATE managed_sessions
             SET attach_count = attach_count + 1,
                 last_activity_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(session_id)
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            "INSERT INTO session_events (session_id, event_type, message, payload_json)
             VALUES (?, 'attach', ?, ?)",
        )
        .bind(session_id)
        .bind(Some("terminal attached"))
        .bind(Some(serde_json::json!({ "transport": "pty" }).to_string()))
        .execute(&mut *conn)
        .await?;

        let updated = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
        Ok(updated)
    }

    pub async fn detach_terminal_session(&self, session_id: i64) -> Result<ManagedSession> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE managed_sessions
             SET attach_count = CASE WHEN attach_count > 0 THEN attach_count - 1 ELSE 0 END,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(session_id)
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            "INSERT INTO session_events (session_id, event_type, message, payload_json)
             VALUES (?, 'detach', ?, ?)",
        )
        .bind(session_id)
        .bind(Some("terminal detached"))
        .bind(Some(serde_json::json!({ "transport": "pty" }).to_string()))
        .execute(&mut *conn)
        .await?;

        let updated = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
        Ok(updated)
    }

    pub async fn set_session_pid(&self, session_id: i64, pid: Option<i64>) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET pid = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(pid)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn end_session(&self, session_id: i64, reason: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE managed_sessions SET status = 'ended', failure_reason = ?, ended_at = CURRENT_TIMESTAMP, needs_input = 0, input_reason = NULL, attach_count = 0, last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(reason)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE agents
             SET active_session_id = (
                 SELECT ms.id
                 FROM managed_sessions ms
                 WHERE ms.agent_id = agents.id
                   AND ms.id <> ?
                   AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
                 ORDER BY COALESCE(ms.last_activity_at, ms.last_heartbeat_at, ms.updated_at) DESC, ms.id DESC
                 LIMIT 1
             ),
             updated_at = CURRENT_TIMESTAMP
             WHERE active_session_id = ?",
        )
            .bind(session_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_managed_session(&self, session_id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM managed_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(anyhow!("session not found"));
        }

        sqlx::query(
            "UPDATE agents
             SET active_session_id = (
                 SELECT ms.id
                 FROM managed_sessions ms
                 WHERE ms.agent_id = agents.id
                   AND ms.id <> ?
                   AND ms.status IN ('waking', 'active', 'stalled', 'needs_input')
                 ORDER BY COALESCE(ms.last_activity_at, ms.last_heartbeat_at, ms.updated_at) DESC, ms.id DESC
                 LIMIT 1
             ),
             updated_at = CURRENT_TIMESTAMP
             WHERE active_session_id = ?",
        )
            .bind(session_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM managed_sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_managed_sessions(
        &self,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<ManagedSession>> {
        let max = limit.unwrap_or(50);
        let sessions = if let Some(status) = status {
            sqlx::query_as::<_, ManagedSession>(&format!(
                "SELECT {} FROM managed_sessions WHERE status = ? ORDER BY updated_at DESC LIMIT ?",
                MANAGED_SESSION_SELECT_COLUMNS
            ))
            .bind(status)
            .bind(max)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, ManagedSession>(&format!(
                "SELECT {} FROM managed_sessions ORDER BY updated_at DESC LIMIT ?",
                MANAGED_SESSION_SELECT_COLUMNS
            ))
            .bind(max)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(sessions)
    }

    pub async fn get_managed_session(&self, session_id: i64) -> Result<ManagedSession> {
        let session = sqlx::query_as::<_, ManagedSession>(&format!(
            "SELECT {} FROM managed_sessions WHERE id = ?",
            MANAGED_SESSION_SELECT_COLUMNS
        ))
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(session)
    }

    pub async fn insert_session_event(
        &self,
        session_id: i64,
        event_type: &str,
        message: Option<&str>,
        payload_json: Option<&str>,
    ) -> Result<SessionEvent> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "INSERT INTO session_events (session_id, event_type, message, payload_json) VALUES (?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(event_type)
        .bind(message)
        .bind(payload_json)
        .execute(&mut *conn)
        .await?;

        let event = sqlx::query_as::<_, SessionEvent>(
            "SELECT id, session_id, event_type, message, payload_json, created_at FROM session_events WHERE id = last_insert_rowid()",
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(event)
    }

    pub async fn create_session_alert(
        &self,
        session_id: i64,
        agent_id: Option<i64>,
        severity: &str,
        reason: &str,
        message: &str,
        requires_ack: bool,
    ) -> Result<SessionAlert> {
        self.create_session_alert_with_enrichment(
            session_id,
            agent_id,
            severity,
            reason,
            message,
            requires_ack,
            AlertEnrichmentInput::default(),
        )
        .await
    }

    pub async fn create_session_alert_with_enrichment(
        &self,
        session_id: i64,
        agent_id: Option<i64>,
        severity: &str,
        reason: &str,
        message: &str,
        requires_ack: bool,
        enrichment: AlertEnrichmentInput,
    ) -> Result<SessionAlert> {
        let mut conn = self.pool.acquire().await?;
        let linked_agent_id = match agent_id {
            Some(id) => Some(id),
            None => sqlx::query_scalar::<_, Option<i64>>(
                "SELECT agent_id FROM managed_sessions WHERE id = ?",
            )
            .bind(session_id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten(),
        };

        let existing_alert_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM session_alerts
             WHERE session_id = ?
               AND reason = ?
               AND message = ?
               AND resolved_at IS NULL
             ORDER BY id DESC
             LIMIT 1",
        )
        .bind(session_id)
        .bind(reason)
        .bind(message)
        .fetch_optional(&mut *conn)
        .await?;

        let enrichment_status = enrichment
            .message_enrichment_status
            .unwrap_or_else(|| "pending".to_string());
        let enrichment_output = enrichment.message_enriched.and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        });
        let enrichment_error = enrichment.message_enrichment_error;
        let enriched_at = if enrichment_output.is_some() {
            Some("CURRENT_TIMESTAMP")
        } else {
            None
        };

        let alert = if let Some(alert_id) = existing_alert_id {
            sqlx::query(
                "UPDATE session_alerts
                 SET severity = ?,
                     requires_ack = ?,
                     agent_id = COALESCE(agent_id, ?),
                     message_enriched = ?,
                     message_enrichment_status = ?,
                     message_enrichment_error = ?,
                     message_enriched_at = CASE WHEN ? IS NOT NULL THEN CURRENT_TIMESTAMP ELSE NULL END,
                     snoozed_until = NULL,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?",
            )
            .bind(severity)
            .bind(requires_ack)
            .bind(linked_agent_id)
            .bind(enrichment_output.as_deref())
            .bind(&enrichment_status)
            .bind(enrichment_error.as_deref())
            .bind(enriched_at)
            .bind(alert_id)
            .execute(&mut *conn)
            .await?;

            sqlx::query_as::<_, SessionAlert>(&format!(
                "SELECT {} FROM session_alerts WHERE id = ?",
                SESSION_ALERT_SELECT_COLUMNS
            ))
            .bind(alert_id)
            .fetch_one(&mut *conn)
            .await?
        } else {
            sqlx::query(
                "INSERT INTO session_alerts (
                    session_id,
                    agent_id,
                    severity,
                    reason,
                    message,
                    message_enriched,
                    message_enrichment_status,
                    message_enrichment_error,
                    message_enriched_at,
                    requires_ack
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, CASE WHEN ? IS NOT NULL THEN CURRENT_TIMESTAMP ELSE NULL END, ?)",
            )
            .bind(session_id)
            .bind(linked_agent_id)
            .bind(severity)
            .bind(reason)
            .bind(message)
            .bind(enrichment_output.as_deref())
            .bind(&enrichment_status)
            .bind(enrichment_error.as_deref())
            .bind(enriched_at)
            .bind(requires_ack)
            .execute(&mut *conn)
            .await?;

            sqlx::query_as::<_, SessionAlert>(&format!(
                "SELECT {} FROM session_alerts WHERE id = last_insert_rowid()",
                SESSION_ALERT_SELECT_COLUMNS
            ))
            .fetch_one(&mut *conn)
            .await?
        };

        if let Some(agent_id) = alert.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }

        sqlx::query(
            "INSERT INTO session_events (session_id, event_type, message, payload_json)
             VALUES (?, 'session_alert_upserted', ?, ?)",
        )
        .bind(session_id)
        .bind(Some("structured alert persisted"))
        .bind(Some(
            serde_json::json!({
                "alertId": alert.id,
                "agentId": alert.agent_id,
                "severity": alert.severity,
                "reason": alert.reason,
                "message": alert.message,
                "messageEnriched": alert.message_enriched,
                "messageEnrichmentStatus": alert.message_enrichment_status,
                "requiresAck": alert.requires_ack
            })
            .to_string(),
        ))
        .execute(&mut *conn)
        .await?;

        Ok(alert)
    }

    pub async fn get_session_alert(&self, alert_id: i64) -> Result<SessionAlert> {
        let alert = sqlx::query_as::<_, SessionAlert>(&format!(
            "SELECT {} FROM session_alerts WHERE id = ?",
            SESSION_ALERT_SELECT_COLUMNS
        ))
        .bind(alert_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(alert)
    }

    pub async fn update_session_alert_enrichment(
        &self,
        alert_id: i64,
        message_enriched: Option<&str>,
        message_enrichment_status: &str,
        message_enrichment_error: Option<&str>,
    ) -> Result<SessionAlert> {
        sqlx::query(
            "UPDATE session_alerts
             SET message_enriched = ?,
                 message_enrichment_status = ?,
                 message_enrichment_error = ?,
                 message_enriched_at = CASE WHEN ? IS NOT NULL THEN CURRENT_TIMESTAMP ELSE NULL END,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(message_enriched)
        .bind(message_enrichment_status)
        .bind(message_enrichment_error)
        .bind(message_enriched)
        .bind(alert_id)
        .execute(&self.pool)
        .await?;
        self.get_session_alert(alert_id).await
    }

    pub async fn acknowledge_session_alert(&self, alert_id: i64) -> Result<SessionAlert> {
        sqlx::query(
            "UPDATE session_alerts SET acknowledged_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(alert_id)
        .execute(&self.pool)
        .await?;
        self.get_session_alert(alert_id).await
    }

    pub async fn snooze_session_alert(
        &self,
        alert_id: i64,
        duration_minutes: i64,
    ) -> Result<SessionAlert> {
        let clamped_minutes = duration_minutes.max(1).min(24 * 60);
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE session_alerts
             SET snoozed_until = datetime(CURRENT_TIMESTAMP, '+' || ? || ' minutes'),
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(clamped_minutes)
        .bind(alert_id)
        .execute(&mut *conn)
        .await?;

        let alert = sqlx::query_as::<_, SessionAlert>(&format!(
            "SELECT {} FROM session_alerts WHERE id = ?",
            SESSION_ALERT_SELECT_COLUMNS
        ))
        .bind(alert_id)
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = alert.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }

        Ok(alert)
    }

    pub async fn escalate_session_alert(&self, alert_id: i64) -> Result<SessionAlert> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE session_alerts
             SET severity = 'critical',
                 escalated_at = CURRENT_TIMESTAMP,
                 escalation_count = escalation_count + 1,
                 snoozed_until = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(alert_id)
        .execute(&mut *conn)
        .await?;

        let alert = sqlx::query_as::<_, SessionAlert>(&format!(
            "SELECT {} FROM session_alerts WHERE id = ?",
            SESSION_ALERT_SELECT_COLUMNS
        ))
        .bind(alert_id)
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = alert.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }

        Ok(alert)
    }

    pub async fn resolve_session_alert(&self, alert_id: i64) -> Result<SessionAlert> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query(
            "UPDATE session_alerts SET resolved_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(alert_id)
        .execute(&mut *conn)
        .await?;

        let alert = sqlx::query_as::<_, SessionAlert>(&format!(
            "SELECT {} FROM session_alerts WHERE id = ?",
            SESSION_ALERT_SELECT_COLUMNS
        ))
        .bind(alert_id)
        .fetch_one(&mut *conn)
        .await?;

        if let Some(agent_id) = alert.agent_id {
            Self::refresh_agent_attention_state_with_conn(&mut conn, agent_id).await?;
        }

        Ok(alert)
    }

    pub async fn alert_resolution_latency_ms(&self, alert_id: i64) -> Result<Option<i64>> {
        let latency = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT
                CASE
                    WHEN resolved_at IS NULL THEN NULL
                    ELSE CAST((julianday(resolved_at) - julianday(created_at)) * 86400000 AS INTEGER)
                END
             FROM session_alerts
             WHERE id = ?",
        )
        .bind(alert_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(latency)
    }

    pub async fn list_session_events(
        &self,
        session_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<SessionEvent>> {
        let max = limit.unwrap_or(100);
        let events = sqlx::query_as::<_, SessionEvent>(
            "SELECT id, session_id, event_type, message, payload_json, created_at FROM session_events WHERE session_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(session_id)
        .bind(max)
        .fetch_all(&self.pool)
        .await?;
        Ok(events)
    }

    pub async fn list_unresolved_session_alerts(
        &self,
        agent_id: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Vec<SessionAlert>> {
        self.list_session_alerts(agent_id, true, limit).await
    }

    pub async fn list_session_alerts(
        &self,
        agent_id: Option<i64>,
        unresolved_only: bool,
        limit: Option<i64>,
    ) -> Result<Vec<SessionAlert>> {
        let max = limit.unwrap_or(100);
        let alerts = match (agent_id, unresolved_only) {
            (Some(agent_id), true) => {
                sqlx::query_as::<_, SessionAlert>(&format!(
                    "SELECT {} FROM session_alerts
                     WHERE resolved_at IS NULL
                       AND (snoozed_until IS NULL OR snoozed_until <= CURRENT_TIMESTAMP)
                       AND agent_id = ?
                     ORDER BY created_at DESC
                     LIMIT ?",
                    SESSION_ALERT_SELECT_COLUMNS
                ))
                .bind(agent_id)
                .bind(max)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(agent_id), false) => {
                sqlx::query_as::<_, SessionAlert>(&format!(
                "SELECT {} FROM session_alerts WHERE agent_id = ? ORDER BY created_at DESC LIMIT ?",
                SESSION_ALERT_SELECT_COLUMNS
            ))
                .bind(agent_id)
                .bind(max)
                .fetch_all(&self.pool)
                .await?
            }
            (None, true) => {
                sqlx::query_as::<_, SessionAlert>(&format!(
                    "SELECT {} FROM session_alerts
                     WHERE resolved_at IS NULL
                       AND (snoozed_until IS NULL OR snoozed_until <= CURRENT_TIMESTAMP)
                     ORDER BY created_at DESC
                     LIMIT ?",
                    SESSION_ALERT_SELECT_COLUMNS
                ))
                .bind(max)
                .fetch_all(&self.pool)
                .await?
            }
            (None, false) => {
                sqlx::query_as::<_, SessionAlert>(&format!(
                    "SELECT {} FROM session_alerts ORDER BY created_at DESC LIMIT ?",
                    SESSION_ALERT_SELECT_COLUMNS
                ))
                .bind(max)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(alerts)
    }

    pub async fn get_runtime_issue(&self, kind: &str) -> Result<RuntimeIssue> {
        let issue = sqlx::query_as::<_, RuntimeIssue>(&format!(
            "SELECT {} FROM runtime_issues WHERE kind = ?",
            RUNTIME_ISSUE_SELECT_COLUMNS
        ))
        .bind(kind)
        .fetch_one(&self.pool)
        .await?;
        Ok(issue)
    }

    pub async fn report_runtime_issue(
        &self,
        kind: &str,
        source: &str,
        raw_message: &str,
        enriched_message: Option<&str>,
        enrichment_status: &str,
        enrichment_error: Option<&str>,
    ) -> Result<RuntimeIssue> {
        sqlx::query(
            "INSERT INTO runtime_issues (
                kind,
                source,
                raw_message,
                enriched_message,
                enrichment_status,
                enrichment_error,
                first_seen_at,
                last_seen_at,
                seen_count,
                dismissed_until,
                resolved_at
            ) VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 1, NULL, NULL)
            ON CONFLICT(kind) DO UPDATE SET
                source = excluded.source,
                raw_message = excluded.raw_message,
                enriched_message = excluded.enriched_message,
                enrichment_status = excluded.enrichment_status,
                enrichment_error = excluded.enrichment_error,
                last_seen_at = CURRENT_TIMESTAMP,
                seen_count = runtime_issues.seen_count + 1,
                resolved_at = NULL",
        )
        .bind(kind)
        .bind(source)
        .bind(raw_message)
        .bind(enriched_message)
        .bind(enrichment_status)
        .bind(enrichment_error)
        .execute(&self.pool)
        .await?;
        self.get_runtime_issue(kind).await
    }

    pub async fn list_visible_runtime_issues(
        &self,
        limit: Option<i64>,
    ) -> Result<Vec<RuntimeIssue>> {
        let max = limit.unwrap_or(100);
        let rows = sqlx::query_as::<_, RuntimeIssue>(&format!(
            "SELECT {} FROM runtime_issues
             WHERE resolved_at IS NULL
               AND (dismissed_until IS NULL OR dismissed_until <= CURRENT_TIMESTAMP)
             ORDER BY last_seen_at DESC
             LIMIT ?",
            RUNTIME_ISSUE_SELECT_COLUMNS
        ))
        .bind(max)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn dismiss_runtime_issue(
        &self,
        kind: &str,
        duration_ms: i64,
    ) -> Result<RuntimeIssue> {
        let seconds = (duration_ms.max(0) + 999) / 1000;
        sqlx::query(
            "UPDATE runtime_issues
             SET dismissed_until = datetime(CURRENT_TIMESTAMP, '+' || ? || ' seconds')
             WHERE kind = ?",
        )
        .bind(seconds)
        .bind(kind)
        .execute(&self.pool)
        .await?;
        self.get_runtime_issue(kind).await
    }

    pub async fn clear_runtime_issue(&self, kind: &str) -> Result<()> {
        sqlx::query(
            "UPDATE runtime_issues
             SET resolved_at = CURRENT_TIMESTAMP,
                 dismissed_until = NULL
             WHERE kind = ?",
        )
        .bind(kind)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_linear_issue(
        &self,
        id: &str,
        title: &str,
        state: Option<&str>,
        url: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO linear_issues (id, title, state, url) VALUES (?, ?, ?, ?)\n             ON CONFLICT(id) DO UPDATE SET title = excluded.title, state = excluded.state, url = excluded.url, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(id)
        .bind(title)
        .bind(state)
        .bind(url)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_linear_issue(&self, id: &str) -> Result<(String, String)> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT id, title FROM linear_issues WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests;
