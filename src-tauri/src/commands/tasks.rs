use crate::commands::emit_task_updated;
use crate::db::{models::Task, Db};

pub async fn create_task(db: &Db, title: String, state: Option<String>) -> anyhow::Result<Task> {
    let task = db.create_task(&title, state.as_deref()).await?;
    Ok(task)
}

pub async fn update_task(
    db: &Db,
    id: i64,
    title: Option<String>,
    state: Option<String>,
) -> anyhow::Result<Task> {
    let task = db
        .update_task(id, title.as_deref(), state.as_deref())
        .await?;
    Ok(task)
}

pub async fn delete_task(db: &Db, id: i64) -> anyhow::Result<()> {
    db.delete_task(id).await?;
    Ok(())
}

pub async fn move_task_state(db: &Db, id: i64, state: String) -> anyhow::Result<Task> {
    let task = db.move_task_state(id, &state).await?;
    Ok(task)
}

pub async fn list_tasks(db: &Db) -> anyhow::Result<Vec<Task>> {
    let tasks = db.list_tasks().await?;
    Ok(tasks)
}

#[tauri::command]
pub async fn create_task_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    title: String,
    state: Option<String>,
) -> Result<Task, String> {
    let task = create_task(&db, title, state)
        .await
        .map_err(|e| e.to_string())?;
    let _ = emit_task_updated(&app, task.id);
    Ok(task)
}

#[tauri::command]
pub async fn update_task_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    id: i64,
    title: Option<String>,
    state: Option<String>,
) -> Result<Task, String> {
    let task = update_task(&db, id, title, state)
        .await
        .map_err(|e| e.to_string())?;
    let _ = emit_task_updated(&app, task.id);
    Ok(task)
}

#[tauri::command]
pub async fn delete_task_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    id: i64,
) -> Result<(), String> {
    delete_task(&db, id).await.map_err(|e| e.to_string())?;
    let _ = emit_task_updated(&app, id);
    Ok(())
}

#[tauri::command]
pub async fn move_task_state_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    id: i64,
    state: String,
) -> Result<Task, String> {
    let task = move_task_state(&db, id, state)
        .await
        .map_err(|e| e.to_string())?;
    let _ = emit_task_updated(&app, task.id);
    Ok(task)
}

#[tauri::command]
pub async fn list_tasks_cmd(db: tauri::State<'_, Db>) -> Result<Vec<Task>, String> {
    list_tasks(&db).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_task_command() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let task = create_task(&db, "Write tests".to_string(), None)
            .await
            .expect("task");
        assert_eq!(task.title, "Write tests");
    }

    #[tokio::test]
    async fn list_tasks_command() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let _ = create_task(&db, "First".to_string(), None)
            .await
            .expect("task");
        let _ = create_task(&db, "Second".to_string(), None)
            .await
            .expect("task");
        let tasks = list_tasks(&db).await.expect("list");
        assert!(tasks.len() >= 2);
    }
}
