use crate::db::Db;
use anyhow::Result;

pub struct LinearImporter {
    db: Db,
}

impl LinearImporter {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn import_issue(
        &self,
        id: &str,
        title: &str,
        state: Option<&str>,
        url: Option<&str>,
    ) -> Result<()> {
        // Stubbed: in a real implementation this would call the Linear API.
        self.db.upsert_linear_issue(id, title, state, url).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
