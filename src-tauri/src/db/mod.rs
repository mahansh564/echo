pub mod models;

use anyhow::Result;
use models::{ManagedSession, SessionEvent, Task};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
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
        state: Option<&str>,
        task_id: Option<i64>,
    ) -> Result<models::Agent> {
        let mut conn = self.pool.acquire().await?;
        let state_value = state.unwrap_or("idle");
        sqlx::query("INSERT INTO agents (name, state, task_id) VALUES (?, ?, ?)")
            .bind(name)
            .bind(state_value)
            .bind(task_id)
            .execute(&mut *conn)
            .await?;

        let agent = sqlx::query_as::<_, models::Agent>(
            "SELECT id, name, state, task_id, last_snippet, updated_at FROM agents WHERE id = last_insert_rowid()",
        )
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

        let agent = sqlx::query_as::<_, models::Agent>(
            "SELECT id, name, state, task_id, last_snippet, updated_at FROM agents WHERE id = ?",
        )
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
        let agents = sqlx::query_as::<_, models::Agent>(
            "SELECT id, name, state, task_id, last_snippet, updated_at FROM agents ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(agents)
    }

    pub async fn update_agent_snippet(&self, agent_id: i64, snippet: &str) -> Result<()> {
        sqlx::query("UPDATE agents SET last_snippet = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
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

        let session = sqlx::query_as::<_, ManagedSession>(
            "SELECT id, provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, failure_reason, metadata_json, created_at, updated_at FROM managed_sessions WHERE id = last_insert_rowid()",
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(session)
    }

    pub async fn update_session_status(
        &self,
        session_id: i64,
        status: &str,
        failure_reason: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET status = ?, failure_reason = ?, started_at = CASE WHEN ? = 'active' AND started_at IS NULL THEN CURRENT_TIMESTAMP ELSE started_at END, ended_at = CASE WHEN ? IN ('ended', 'failed') THEN CURRENT_TIMESTAMP ELSE ended_at END, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(status)
        .bind(failure_reason)
        .bind(status)
        .bind(status)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_session_heartbeat(&self, session_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET last_heartbeat_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
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

    pub async fn set_session_pid(&self, session_id: i64, pid: Option<i64>) -> Result<()> {
        sqlx::query("UPDATE managed_sessions SET pid = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(pid)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn end_session(&self, session_id: i64, reason: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE managed_sessions SET status = 'ended', failure_reason = ?, ended_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(reason)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_managed_sessions(
        &self,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<ManagedSession>> {
        let max = limit.unwrap_or(50);
        let sessions = if let Some(status) = status {
            sqlx::query_as::<_, ManagedSession>(
                "SELECT id, provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, failure_reason, metadata_json, created_at, updated_at FROM managed_sessions WHERE status = ? ORDER BY updated_at DESC LIMIT ?",
            )
            .bind(status)
            .bind(max)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, ManagedSession>(
                "SELECT id, provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, failure_reason, metadata_json, created_at, updated_at FROM managed_sessions ORDER BY updated_at DESC LIMIT ?",
            )
            .bind(max)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(sessions)
    }

    pub async fn get_managed_session(&self, session_id: i64) -> Result<ManagedSession> {
        let session = sqlx::query_as::<_, ManagedSession>(
            "SELECT id, provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, failure_reason, metadata_json, created_at, updated_at FROM managed_sessions WHERE id = ?",
        )
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
        let row =
            sqlx::query_as::<_, (String, String)>("SELECT id, title FROM linear_issues WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests;
