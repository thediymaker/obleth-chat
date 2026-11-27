//! Real database checks for preservation, ownership, atomic bulk operations, and imports.
mod common;
use axum::http::StatusCode;
use common::Chats;
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
/// Checks a chat's messages form one path in order, ending at its selected leaf.
async fn assert_linked(pool: &PgPool, conversation: Uuid) -> Vec<Uuid> {
    let rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT id,parent_message_id,selected_child_id FROM messages WHERE conversation_id=$1 ORDER BY ordinal",
    )
    .bind(conversation)
    .fetch_all(pool)
    .await
    .unwrap();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    for (i, (_, parent, child)) in rows.iter().enumerate() {
        assert_eq!(*parent, i.checked_sub(1).map(|p| ids[p]), "parent of {i}");
        assert_eq!(*child, ids.get(i + 1).copied(), "child of {i}");
    }
    let leaf: Option<Uuid> =
        sqlx::query_scalar("SELECT selected_leaf_id FROM conversations WHERE id=$1")
            .bind(conversation)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(leaf, ids.last().copied());
    ids
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
async fn the_settings_migration_leaves_a_chat_its_parameters_only(pool: PgPool) {
    let alice = user(&pool).await;
    let s = Store::new(pool.clone());
    let set = |id: Uuid, settings: serde_json::Value| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE conversations SET settings = $2 WHERE id = $1")
                .bind(id)
                .bind(settings)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let read = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT settings FROM conversations WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // As 1.9 left a chat after a switch to a version Auto had written.
    let polluted = s.create(alice, "Polluted", "fixture").await.unwrap();
    set(
        polluted.id,
        json!({"temperature": 0.2, "systemPrompt": "Be brief", "auto": true, "autoWhy": "a short question", "compare": ["a", "b"]}),
    )
    .await;
    // Only what does not belong: nothing is left.
    let only = s.create(alice, "Only extras", "fixture").await.unwrap();
    set(only.id, json!({"auto": true, "autoWhy": "x"})).await;
    let clean = s.create(alice, "Clean", "fixture").await.unwrap();
    let parameters = json!({"systemPrompt": "Be brief", "temperature": 0.2, "topP": 0.9, "maxTokens": 512, "reasoningEffort": "high"});
    set(clean.id, parameters.clone()).await;
    let untouched = s.create(alice, "As made", "fixture").await.unwrap();
    let as_made = read(untouched.id).await;
    // The export of such a chat failed before the migration.
    assert!(s.export(alice, &[polluted.id]).await.is_err());

    let migration = include_str!("../migrations/101_chat_settings_only.sql");
    let changed = sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    assert_eq!(changed.rows_affected(), 2);
    assert_eq!(
        read(polluted.id).await,
        json!({"temperature": 0.2, "systemPrompt": "Be brief"})
    );
    assert_eq!(read(only.id).await, json!({}));
    assert_eq!(read(clean.id).await, parameters);
    assert_eq!(read(untouched.id).await, as_made);
    s.export(alice, &[polluted.id]).await.unwrap();
    s.get(alice, only.id).await.unwrap();
    // A second run finds nothing to do.
    let again = sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    assert_eq!(again.rows_affected(), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_path_migration_links_chats_left_unlinked(pool: PgPool) {
    let alice = user(&pool).await;
    let s = Store::new(pool.clone());
    let linked = s.create(alice, "Linked", "fixture").await.unwrap();
    exchange(&s, alice, linked.id, "question", "answer").await;
    let before = assert_linked(&pool, linked.id).await;
    // Written as older imports and copies left them: no links and no leaf.
    let old = s.create(alice, "Old import", "fixture").await.unwrap();
    for (ordinal, role) in ["user", "assistant", "user", "assistant"]
        .into_iter()
        .enumerate()
    {
        sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status,generation_id) VALUES($1,$2,$3,$4,$5,'completed',$6)")
            .bind(Uuid::new_v4())
            .bind(old.id)
            .bind(ordinal as i32)
            .bind(role)
            .bind(format!("message {ordinal}"))
            .bind((role == "assistant").then(Uuid::new_v4))
            .execute(&pool)
            .await
            .unwrap();
    }
    let empty = s.create(alice, "Empty", "fixture").await.unwrap();
    let migration = include_str!("../migrations/087_initialize_paths.sql");
    sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    let ids = assert_linked(&pool, old.id).await;
    assert_eq!(ids.len(), 4);
    let chat = s.get(alice, old.id).await.unwrap();
    let contents: Vec<&str> = chat.messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        ["message 0", "message 1", "message 2", "message 3"]
    );
    assert_eq!(assert_linked(&pool, linked.id).await, before);
    assert!(assert_linked(&pool, empty.id).await.is_empty());
    // A second run finds nothing to do.
    sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    assert_eq!(assert_linked(&pool, old.id).await, ids);
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
    assert_linked(&s.pool, fork.conversation.id).await;
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
    assert_eq!(assert_linked(&s.pool, copy.id).await.len(), 2);
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
    // What Chat itself writes on a reply comes back in: that Auto chose it, a
    // comparison, and the timings beside the token counts. Only the model and the
    // parameters are kept as its settings.
    let mut archive = s.export(alice, &[c.id]).await.unwrap();
    archive.conversations[0].messages[1].settings = Some(json!({
        "model": "served-model", "temperature": 0.5, "auto": true,
        "autoWhy": {"reason": "close", "next": [{"model": "other"}]}, "compare": {"with": []},
    }));
    archive.conversations[0].messages[1].usage = Some(json!({
        "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15, "steps": 2,
        "generation_ms": 900, "first_token_ms": 120, "last_prompt_tokens": 10,
        "last_completion_tokens": 5, "energy_wh": 0.25, "co2_g": 0.1, "later": "x",
    }));
    let copy = s.import(alice, archive).await.unwrap();
    let kept = s.get(alice, copy[0].id).await.unwrap().messages[1].clone();
    assert_eq!(
        kept.settings,
        Some(json!({"model": "served-model", "temperature": 0.5}))
    );
    let usage = kept.usage.unwrap();
    assert_eq!(
        (&usage["steps"], &usage["energy_wh"]),
        (&json!(2), &json!(0.25))
    );
    assert!(usage.get("later").is_none());
    for usage in [json!([]), json!({"steps": "2"}), json!({"energy_wh": -1})] {
        let mut archive = s.export(alice, &[c.id]).await.unwrap();
        archive.conversations[0].messages[1].usage = Some(usage);
        assert_eq!(
            s.import(alice, archive).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
    // A reply Auto wrote can be retried: the new chat takes its parameters only.
    sqlx::query("UPDATE messages SET settings=$2 WHERE id=$1")
        .bind(answer)
        .bind(json!({"model": "fixture", "temperature": 0.7, "auto": true, "autoWhy": {"reason": "close"}}))
        .execute(&pool)
        .await
        .unwrap();
    let fork = s
        .fork_turn(alice, c.id, answer, "regenerate", None)
        .await
        .unwrap();
    assert_eq!(fork.settings, json!({"temperature": 0.7}));
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

#[sqlx::test(migrations = "./migrations")]
async fn the_list_says_which_chats_are_working_and_which_wait_for_an_answer(pool: PgPool) {
    let alice = user(&pool).await;
    let s = Store::new(pool.clone());
    let quiet = s.create(alice, "Quiet", "fixture").await.unwrap();
    let busy = s.create(alice, "Busy", "fixture").await.unwrap();
    let turn = s
        .start_turn(alice, busy.id, "Run the sweep", "fixture", "", 0.7, 2048)
        .await
        .unwrap();
    let flags = |id: Uuid| {
        let s = &s;
        async move {
            let page = s.history(alice, HistoryQuery::default()).await.unwrap();
            let chat = page.conversations.iter().find(|c| c.id == id).unwrap();
            (chat.working, chat.waiting, chat.waiting_tool.clone())
        }
    };
    let asked = Some("submit_job".to_owned());
    assert_eq!(flags(quiet.id).await, (false, false, None));
    assert_eq!(flags(busy.id).await, (true, false, None));

    sqlx::query("INSERT INTO tool_approvals(generation_id,call_id,conversation_id,owner_id,tool_name) VALUES($1,'call-1',$2,$3,'submit_job')")
        .bind(turn.generation_id)
        .bind(busy.id)
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(flags(busy.id).await, (true, true, asked));
    sqlx::query("UPDATE tool_approvals SET decision='allow' WHERE generation_id=$1")
        .bind(turn.generation_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(flags(busy.id).await, (true, false, None));

    // A question left over from a reply that has ended does not count.
    sqlx::query("UPDATE tool_approvals SET decision=NULL WHERE generation_id=$1")
        .bind(turn.generation_id)
        .execute(&pool)
        .await
        .unwrap();
    s.finish_turn_detailed(
        alice,
        busy.id,
        turn.generation_id,
        "Done.",
        "",
        "completed",
        None,
        None,
        Some("stop"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(flags(busy.id).await, (false, false, None));
}
