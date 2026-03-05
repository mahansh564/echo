use super::intent::parse_intent;
use super::resolver::resolve_status_query;
use super::router::{resolve_agent_id_from_payload, AgentLocator};
use serde_json::json;

#[tokio::test]
async fn intent_parses() {
    let intent = parse_intent("http://localhost:9", "status of agent 3")
        .await
        .expect("intent");
    assert_eq!(intent.action, "status_agent");
    assert_eq!(
        intent
            .payload
            .get("agent_index")
            .and_then(|value| value.as_i64()),
        Some(3)
    );
}

#[tokio::test]
async fn intent_parses_nato_alias_target() {
    let intent = parse_intent("http://localhost:9", "status of agent alpha")
        .await
        .expect("intent");
    assert_eq!(intent.action, "status_agent");
    assert_eq!(
        intent
            .payload
            .get("agent_index")
            .and_then(|value| value.as_i64()),
        Some(1)
    );
}

#[test]
fn router_resolves_agent_index_and_alias() {
    let agents = vec![
        AgentLocator {
            id: 10,
            name: "Agent One".to_string(),
            display_order: 1,
        },
        AgentLocator {
            id: 20,
            name: "Agent Two".to_string(),
            display_order: 2,
        },
        AgentLocator {
            id: 30,
            name: "Agent Three".to_string(),
            display_order: 3,
        },
    ];

    let by_index = resolve_agent_id_from_payload(&agents, &json!({ "agent_index": 3 }));
    assert_eq!(by_index, Some(30));

    let by_alias = resolve_agent_id_from_payload(
        &agents,
        &json!({ "agent_alias": "alpha", "query": "status of agent alpha" }),
    );
    assert_eq!(by_alias, Some(10));
}

#[tokio::test]
async fn status_resolver_prefers_deterministic_index() {
    let resolved = resolve_status_query("http://localhost:9", "status of agent 3").await;
    assert_eq!(resolved.agent_index_hint, Some(3));
    assert_eq!(resolved.agent_name_hint, None);
}

#[tokio::test]
async fn status_resolver_prefers_deterministic_alias() {
    let resolved = resolve_status_query("http://localhost:9", "status of agent alpha").await;
    assert_eq!(resolved.agent_index_hint, Some(1));
    assert_eq!(resolved.agent_name_hint.as_deref(), Some("alpha"));
}
