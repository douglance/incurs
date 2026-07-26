use async_trait::async_trait;
use incurs_codemode::{ArtifactRef, ArtifactStore, ExecutionState, RuntimeStore, Snippet};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use worker::{Result, SqlStorage, SqlStorageValue};

/// Durable Object SQLite implementation of the Code Mode store.
#[derive(Clone)]
pub struct DurableSqlStore {
    sql: SqlStorage,
}

#[derive(Deserialize)]
struct JsonRow {
    json: String,
}

impl DurableSqlStore {
    /// Creates the Code Mode tables when absent.
    pub fn new(sql: SqlStorage) -> Result<Self> {
        sql.exec(
            "CREATE TABLE IF NOT EXISTS incurs_cm_executions (
                id TEXT PRIMARY KEY,
                json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS incurs_cm_execution_status
               ON incurs_cm_executions(status, created_at);
             CREATE TABLE IF NOT EXISTS incurs_cm_snippets (
                name TEXT PRIMARY KEY,
                json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS incurs_cm_artifacts (
                id TEXT PRIMARY KEY,
                execution_id TEXT NOT NULL,
                json TEXT NOT NULL
             );",
            None,
        )?;
        Ok(Self { sql })
    }
}

#[async_trait]
impl ArtifactStore for DurableSqlStore {
    async fn put(
        &self,
        execution_id: &str,
        value: &Value,
    ) -> std::result::Result<ArtifactRef, String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        let id = format!(
            "{:x}",
            Sha256::digest([execution_id.as_bytes(), &bytes].concat())
        );
        self.sql
            .exec(
                "INSERT OR REPLACE INTO incurs_cm_artifacts (id, execution_id, json)
                 VALUES (?, ?, ?)",
                vec![
                    id.clone().into(),
                    execution_id.into(),
                    String::from_utf8_lossy(&bytes).into_owned().into(),
                ],
            )
            .map_err(|error| error.to_string())?;
        let mut preview = String::from_utf8_lossy(&bytes).into_owned();
        if preview.len() > 512 {
            let mut end = 512;
            while !preview.is_char_boundary(end) {
                end -= 1;
            }
            preview.truncate(end);
        }
        Ok(ArtifactRef {
            id,
            execution_id: execution_id.to_string(),
            bytes: bytes.len(),
            preview,
        })
    }

    async fn get(
        &self,
        execution_id: &str,
        artifact_id: &str,
    ) -> std::result::Result<Option<Value>, String> {
        let rows = self
            .sql
            .exec(
                "SELECT json FROM incurs_cm_artifacts WHERE id = ? AND execution_id = ?",
                vec![artifact_id.into(), execution_id.into()],
            )
            .map_err(|error| error.to_string())?
            .to_array::<JsonRow>()
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .next()
            .map(|row| serde_json::from_str(&row.json).map_err(|error| error.to_string()))
            .transpose()
    }

    async fn delete_execution(&self, execution_id: &str) -> std::result::Result<(), String> {
        self.sql
            .exec(
                "DELETE FROM incurs_cm_artifacts WHERE execution_id = ?",
                vec![execution_id.into()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[async_trait]
impl RuntimeStore for DurableSqlStore {
    async fn get_execution(&self, id: &str) -> std::result::Result<Option<ExecutionState>, String> {
        let rows = self
            .sql
            .exec(
                "SELECT json FROM incurs_cm_executions WHERE id = ?",
                vec![id.into()],
            )
            .map_err(|error| error.to_string())?
            .to_array::<JsonRow>()
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .next()
            .map(|row| serde_json::from_str(&row.json).map_err(|error| error.to_string()))
            .transpose()
    }

    async fn put_execution(&self, execution: &ExecutionState) -> std::result::Result<(), String> {
        self.sql
            .exec(
                "INSERT OR REPLACE INTO incurs_cm_executions
                   (id, json, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                vec![
                    execution.id.clone().into(),
                    serde_json::to_string(execution)
                        .map_err(|error| error.to_string())?
                        .into(),
                    format!("{:?}", execution.status).to_lowercase().into(),
                    (execution.created_at as i64).into(),
                    (execution.updated_at as i64).into(),
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn list_executions(&self) -> std::result::Result<Vec<ExecutionState>, String> {
        self.sql
            .exec(
                "SELECT json FROM incurs_cm_executions ORDER BY created_at DESC, id DESC",
                None,
            )
            .map_err(|error| error.to_string())?
            .to_array::<JsonRow>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| serde_json::from_str(&row.json).map_err(|error| error.to_string()))
            .collect()
    }

    async fn delete_execution(&self, id: &str) -> std::result::Result<(), String> {
        self.sql
            .exec(
                "DELETE FROM incurs_cm_executions WHERE id = ?",
                vec![id.into()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn get_snippet(&self, name: &str) -> std::result::Result<Option<Snippet>, String> {
        let rows = self
            .sql
            .exec(
                "SELECT json FROM incurs_cm_snippets WHERE name = ?",
                vec![name.into()],
            )
            .map_err(|error| error.to_string())?
            .to_array::<JsonRow>()
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .next()
            .map(|row| serde_json::from_str(&row.json).map_err(|error| error.to_string()))
            .transpose()
    }

    async fn put_snippet(&self, snippet: &Snippet) -> std::result::Result<(), String> {
        self.sql
            .exec(
                "INSERT OR REPLACE INTO incurs_cm_snippets (name, json) VALUES (?, ?)",
                vec![
                    snippet.name.clone().into(),
                    serde_json::to_string(snippet)
                        .map_err(|error| error.to_string())?
                        .into(),
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn list_snippets(&self) -> std::result::Result<Vec<Snippet>, String> {
        self.sql
            .exec("SELECT json FROM incurs_cm_snippets ORDER BY name", None)
            .map_err(|error| error.to_string())?
            .to_array::<JsonRow>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| serde_json::from_str(&row.json).map_err(|error| error.to_string()))
            .collect()
    }

    async fn delete_snippet(&self, name: &str) -> std::result::Result<bool, String> {
        Ok(self
            .sql
            .exec(
                "DELETE FROM incurs_cm_snippets WHERE name = ?",
                vec![SqlStorageValue::from(name)],
            )
            .map_err(|error| error.to_string())?
            .rows_written()
            > 0)
    }
}
