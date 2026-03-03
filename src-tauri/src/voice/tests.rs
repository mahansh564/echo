use super::intent::parse_intent;

#[tokio::test]
async fn intent_parses() {
    let intent = parse_intent("http://localhost:9", "create task write docs")
        .await
        .expect("intent");
    assert_eq!(intent.action, "create_task");
}
