//! Real database checks for preservation, ownership, atomic bulk operations, and imports.
use axum::http::StatusCode;
use obleth_chat::store::{ConversationPatch, HistoryQuery, Store};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name) VALUES($1,$2,'History test')")
        .bind(id)
        .bind(format!("history-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}
async fn exchange(s: &Store, u: Uuid, c: Uuid, question: &str, answer: &str) -> Uuid {
    let turn = s
        .start_turn(u, c, question, "fixture", "", 0.7, 2048)
        .await
        .unwrap();
    s.finish_turn_detailed(
        u,
        c,
        turn.generation_id,
        answer,
        "thinking",
        "completed",
        Some(json!({"prompt_tokens":12,"completion_tokens":3,"total_tokens":15})),
        None,
        Some("length"),
        None,
    )
    .await
    .unwrap();
    turn.assistant_id
}

#[sqlx::test(migrations = "./migrations")]
async fn search_pages_pins_archive_and_isolation(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let s = Store::new(pool);
    let c = s.create(alice, "Old title", "fixture").await.unwrap();
    exchange(
        &s,
        alice,
        c.id,
        "Where is the observatory?",
        "The observatory is north.",
    )
    .await;
    let d = s.create(alice, "New title", "fixture").await.unwrap();
    s.patch(
        alice,
        c.id,
        ConversationPatch {
            pinned: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let p = s
        .history(
            alice,
            HistoryQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.total, 2);
    assert!(p.has_more);
    assert_eq!(p.conversations[0].id, c.id);
    let p = s
        .history(
            alice,
            HistoryQuery {
                query: Some("observatory".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.total, 1);
    assert_eq!(p.conversations[0].id, c.id);
    assert_eq!(
        s.history(
            bob,
            HistoryQuery {
                query: Some("observatory".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .total,
        0
    );
    s.bulk(alice, &[c.id, d.id], "archive").await.unwrap();
    assert_eq!(
        s.history(alice, HistoryQuery::default())
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        s.history(
            alice,
            HistoryQuery {
                archived: Some("true".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .total,
        2
    );
    assert!(s.bulk(bob, &[c.id], "delete").await.is_err());
    s.bulk(alice, &[c.id], "unarchive").await.unwrap();
    assert_eq!(
        s.history(alice, HistoryQuery::default())
            .await
            .unwrap()
            .total,
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn edit_and_regenerate_preserve_original_and_parameters(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let s = Store::new(pool);
    let c = s.create(alice, "Original", "fixture").await.unwrap();
    exchange(&s, alice, c.id, "first", "one").await;
    let answer = exchange(&s, alice, c.id, "second", "two").await;
    let original = s.get(alice, c.id).await.unwrap();
    let fork = s
        .fork_turn(alice, c.id, answer, "regenerate", None)
        .await
        .unwrap();
    assert_eq!(fork.draft, "second");
    assert_eq!(fork.conversation.messages.len(), 2);
    assert_eq!(fork.conversation.parent_id, Some(c.id));
    assert_eq!(fork.settings["temperature"], 0.7);
    assert_eq!(fork.conversation.messages[1].reasoning_content, "thinking");
    assert_eq!(
        fork.conversation.messages[1].finish_reason.as_deref(),
        Some("length")
    );
    assert_ne!(fork.conversation.messages[0].id, original.messages[0].id);
    let edited = s
        .fork_turn(
            alice,
            c.id,
            original.messages[0].id,
            "edit",
            Some("revised"),
        )
        .await
        .unwrap();
    assert_eq!(edited.draft, "revised");
    assert!(edited.conversation.messages.is_empty());
    assert_eq!(
        s.get(alice, c.id).await.unwrap().messages[2].content,
        "second"
    );
    assert_eq!(
        s.branches(alice, fork.conversation.id).await.unwrap().len(),
        3
    );
    assert!(s
        .fork_turn(bob, c.id, answer, "regenerate", None)
        .await
        .is_err());
    assert!(s.branches(bob, c.id).await.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn bulk_is_atomic_and_portable_import_creates_private_copies(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let s = Store::new(pool);
    let c = s.create(alice, "Export me", "fixture").await.unwrap();
    exchange(&s, alice, c.id, "question", "answer").await;
    let d = s.create(bob, "Private", "fixture").await.unwrap();
    assert!(s.bulk(alice, &[c.id, d.id], "delete").await.is_err());
    assert!(s.get(alice, c.id).await.is_ok());
    let archive = s.export(alice, &[c.id]).await.unwrap();
    let copies = s.import(bob, archive).await.unwrap();
    assert_ne!(copies[0].id, c.id);
    let copy = s.get(bob, copies[0].id).await.unwrap();
    assert_eq!(copy.messages[1].content, "answer");
    assert_eq!(copy.messages[1].reasoning_content, "thinking");
    assert_eq!(copy.messages[1].finish_reason.as_deref(), Some("length"));
    assert!(s.get(alice, copy.id).await.is_err());
    let mut invalid = s.export(alice, &[c.id]).await.unwrap();
    invalid.conversations[0].messages[1].status = "pending".into();
    assert!(s.import(bob, invalid).await.is_err());
    assert_eq!(s.list(bob).await.unwrap().len(), 2);
    let turn = s
        .start_turn(alice, c.id, "active", "fixture", "", 0.7, 2048)
        .await
        .unwrap();
    assert!(s.bulk(alice, &[c.id], "archive").await.is_err());
    assert!(s.export(alice, &[c.id]).await.is_err());
    s.finish_turn_detailed(
        alice,
        c.id,
        turn.generation_id,
        "partial",
        "",
        "error",
        None,
        None,
        None,
        Some("Gateway stream was interrupted"),
    )
    .await
    .unwrap();
    assert_eq!(
        s.get(alice, c.id)
            .await
            .unwrap()
            .messages
            .last()
            .unwrap()
            .error_message
            .as_deref(),
        Some("Gateway stream was interrupted")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn import_enforces_prompt_length_and_snapshot_settings(pool: PgPool) {
    let alice = user(&pool).await;
    let s = Store::new(pool.clone());
    let c = s.create(alice, "Limits", "fixture").await.unwrap();
    let answer = exchange(&s, alice, c.id, "question", "answer").await;
    // Regeneration reuses an imported prompt, so it must meet the send limit.
    let mut archive = s.export(alice, &[c.id]).await.unwrap();
    archive.conversations[0].messages[0].content = "é".repeat(12_000);
    assert!(s.import(alice, archive).await.is_ok());
    let mut archive = s.export(alice, &[c.id]).await.unwrap();
    archive.conversations[0].messages[0].content = "x".repeat(12_001);
    let error = s.import(alice, archive).await.unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("12,000"));
    for settings in [
        json!([]),
        json!({"model": 5}),
        json!({"model": "bad\u{7}model"}),
        json!({"model": "x".repeat(201)}),
    ] {
        let mut archive = s.export(alice, &[c.id]).await.unwrap();
        archive.conversations[0].messages[1].settings = Some(settings);
        assert_eq!(
            s.import(alice, archive).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut archive = s.export(alice, &[c.id]).await.unwrap();
    archive.conversations[0].messages[1].settings =
        Some(json!({"model": "imported-model", "temperature": 0.5}));
    let copy = s.import(alice, archive).await.unwrap();
    let message = s.get(alice, copy[0].id).await.unwrap().messages[1].id;
    let selected = s.select_version(alice, copy[0].id, message).await.unwrap();
    assert_eq!(selected.model, "imported-model");
    // Snapshots stored before validation cannot push an invalid model or fail.
    for settings in [json!({"model": "bad\u{7}model"}), json!([])] {
        sqlx::query("UPDATE messages SET settings=$2 WHERE id=$1")
            .bind(answer)
            .bind(settings)
            .execute(&pool)
            .await
            .unwrap();
        let selected = s.select_version(alice, c.id, answer).await.unwrap();
        assert_eq!(selected.model, "fixture");
    }
}
