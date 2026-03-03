use super::*;
use crate::db::Db;

#[tokio::test]
async fn linear_import() {
    let db = Db::connect("sqlite::memory:").await.expect("db");
    let importer = LinearImporter::new(db.clone());
    importer
        .import_issue("LIN-1", "Fix bug", Some("Todo"), Some("https://linear.app"))
        .await
        .expect("import");
    let (id, title) = db.get_linear_issue("LIN-1").await.expect("fetch");
    assert_eq!(id, "LIN-1");
    assert_eq!(title, "Fix bug");
}
