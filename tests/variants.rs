//! Regeneration and edits change a selected path, never the sidebar conversation.
use axum::http::StatusCode;
use obleth_chat::{
    store::{ReasoningEffort, Store, MAX_RESPONSE_BYTES},
    MIGRATOR,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name)VALUES($1,$2,'Version test')")
        .bind(id)
        .bind(format!("versions-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}
async fn turn(store: &Store, owner: Uuid, chat: Uuid, prompt: &str, answer: &str) -> Uuid {
    let t = store
        .start_turn(owner, chat, prompt, "fixture", "", 0.7, 256)
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat,
            t.generation_id,
            answer,
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    t.assistant_id
}
async fn variant(
    store: &Store,
    owner: Uuid,
    chat: Uuid,
    source: Uuid,
    mode: &str,
    text: &str,
) -> obleth_chat::store::Turn {
    store
        .start_variant_turn(
            owner,
            chat,
            text,
            "fixture",
            "",
            0.7,
            256,
            Some(ReasoningEffort::High),
            1.0,
            "fixture",
            &[],
            false,
            source,
            mode,
        )
        .await
        .unwrap()
}
#[sqlx::test(migrations = "./migrations")]
async fn regenerated_answers_share_prompt_and_restore_each_versions_continuation(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store
        .create(owner, "One sidebar chat", "fixture")
        .await
        .unwrap()
        .id;
    let first = turn(&store, owner, chat, "Original prompt", "First answer").await;
    let continuation = turn(&store, owner, chat, "First follow-up", "First continuation").await;
    let original = store.get(owner, chat).await.unwrap();
    let prompt_id = original.messages[0].id;
    let next = variant(
        &store,
        owner,
        chat,
        first,
        "regenerate",
        "Cannot replace the saved prompt",
    )
    .await;
    assert_eq!(next.messages.len(), 1);
    assert_eq!(next.messages[0].id, prompt_id);
    assert_eq!(next.messages[0].content, "Original prompt");
    let pending = store.get(owner, chat).await.unwrap();
    assert_eq!(pending.active_generation_id, Some(next.generation_id));
    assert_eq!(pending.messages.len(), 2);
    assert_eq!(pending.messages[1].versions.len(), 2);
    assert_eq!(pending.messages[1].version_index, 2);
    store
        .finish_turn(
            owner,
            chat,
            next.generation_id,
            "Second answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let second_continuation = turn(
        &store,
        owner,
        chat,
        "Second follow-up",
        "Second continuation",
    )
    .await;
    assert_eq!(store.list(owner).await.unwrap().len(), 1);
    let restored = store.select_version(owner, chat, first).await.unwrap();
    assert_eq!(restored.messages.last().unwrap().id, continuation);
    assert_eq!(restored.messages[1].version_index, 1);
    assert_eq!(restored.reasoning_effort, "medium");
    let reloaded = Store::new(pool.clone()).get(owner, chat).await.unwrap();
    assert_eq!(
        reloaded
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Original prompt",
            "First answer",
            "First follow-up",
            "First continuation"
        ]
    );
    let follow = store
        .start_turn(owner, chat, "Continue original", "fixture", "", 0.7, 256)
        .await
        .unwrap();
    assert!(follow
        .messages
        .iter()
        .any(|m| m.content == "First continuation"));
    assert!(!follow.messages.iter().any(|m| m.content == "Second answer"));
    store
        .finish_turn(
            owner,
            chat,
            follow.generation_id,
            "Original extended",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let alternative = store
        .select_version(owner, chat, next.assistant_id)
        .await
        .unwrap();
    assert_eq!(alternative.messages.last().unwrap().id, second_continuation);
    assert_eq!(alternative.messages.len(), 4);
    assert_eq!(alternative.messages[1].version_index, 2);
    let branch = store
        .branch(owner, chat, second_continuation)
        .await
        .unwrap();
    assert_ne!(branch.id, chat);
    assert_eq!(branch.messages.len(), 4);
    assert_eq!(branch.messages[1].content, "Second answer");
    assert_eq!(branch.messages[1].versions.len(), 1);
    assert_eq!(store.list(owner).await.unwrap().len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn edits_keep_original_prompt_versions_and_attachment_context(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let file = Uuid::new_v4();
    sqlx::query("INSERT INTO attachments(id,owner_id,name,mime_type,kind,size,data)VALUES($1,$2,'context.txt','text/plain','text',7,$3)").bind(file).bind(owner).bind(b"context".to_vec()).execute(&pool).await.unwrap();
    let chat = store
        .create(owner, "With file", "fixture")
        .await
        .unwrap()
        .id;
    let initial = store
        .start_turn_attached(
            owner,
            chat,
            "Read it",
            "fixture",
            "",
            0.7,
            256,
            None,
            1.0,
            "fixture",
            &[file],
            false,
        )
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            chat,
            initial.generation_id,
            "Read original",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let original = store.get(owner, chat).await.unwrap();
    let user_message = original.messages[0].id;
    let context = store
        .variant_context(owner, chat, initial.assistant_id, "regenerate")
        .await
        .unwrap();
    assert_eq!(context.draft, "Read it");
    assert_eq!(context.attachments[0].id, file);
    let edited = variant(&store, owner, chat, user_message, "edit", "Summarize it").await;
    assert_ne!(edited.messages.last().unwrap().id, user_message);
    assert_eq!(edited.messages.last().unwrap().content, "Summarize it");
    assert_eq!(
        edited.attachments[&edited.messages.last().unwrap().id][0]
            .meta
            .id,
        file
    );
    store
        .finish_turn(
            owner,
            chat,
            edited.generation_id,
            "Read edited",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let selected = store.get(owner, chat).await.unwrap();
    assert_eq!(selected.messages[0].versions.len(), 2);
    assert_eq!(selected.messages[0].version_index, 2);
    assert_eq!(store.list(owner).await.unwrap().len(), 1);
    let restored = store
        .select_version(owner, chat, user_message)
        .await
        .unwrap();
    assert_eq!(restored.messages[0].content, "Read it");
    assert_eq!(restored.messages[1].content, "Read original");
    assert_eq!(restored.messages[0].attachments[0].id, file);
    let regen = variant(
        &store,
        owner,
        chat,
        initial.assistant_id,
        "regenerate",
        "Read it",
    )
    .await;
    assert_eq!(regen.messages[0].id, user_message);
    assert_eq!(regen.attachments[&user_message][0].meta.id, file);
    store
        .finish_turn(
            owner,
            chat,
            regen.generation_id,
            "Alternative file answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let branch = store.branch(owner, chat, regen.assistant_id).await.unwrap();
    assert_eq!(branch.messages[0].attachments[0].id, file);
    let archive = store.export(owner, &[chat]).await.unwrap();
    let imported = store.import(owner, archive).await.unwrap();
    assert_eq!(
        store.get(owner, imported[0].id).await.unwrap().messages[1].content,
        "Alternative file answer"
    );
    // Clearing the prompt still leaves its saved file attached; neither edit
    // nor regeneration requires the browser to resend the attachment IDs.
    let file_only = variant(&store, owner, chat, user_message, "edit", "").await;
    let file_prompt = file_only.messages.last().unwrap();
    assert!(file_prompt.content.is_empty());
    assert_eq!(file_only.attachments[&file_prompt.id][0].meta.id, file);
    store
        .finish_turn(
            owner,
            chat,
            file_only.generation_id,
            "File-only answer",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let regenerated_file = variant(
        &store,
        owner,
        chat,
        file_only.assistant_id,
        "regenerate",
        "",
    )
    .await;
    assert!(regenerated_file.messages.last().unwrap().content.is_empty());
    assert_eq!(
        regenerated_file.attachments[&file_prompt.id][0].meta.id,
        file
    );
    store
        .finish_turn(
            owner,
            chat,
            regenerated_file.generation_id,
            "File-only alternative",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn variant_privacy_active_guards_interruption_and_failed_admission_are_atomic(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(alice, "Private", "fixture").await.unwrap().id;
    let first = turn(&store, alice, chat, "Prompt", "Saved answer").await;
    assert_eq!(
        store
            .variant_context(bob, chat, first, "regenerate")
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store.select_version(bob, chat, first).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    let different = store
        .create(alice, "Other chat", "fixture")
        .await
        .unwrap()
        .id;
    let foreign_message = turn(&store, alice, different, "Different", "Answer").await;
    assert_eq!(
        store
            .select_version(alice, chat, foreign_message)
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    let pending = variant(&store, alice, chat, first, "regenerate", "Prompt").await;
    assert_eq!(
        store
            .select_version(alice, chat, first)
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        store
            .variant_context(alice, chat, first, "regenerate")
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    store
        .finish_turn_with_reasoning(
            alice,
            chat,
            pending.generation_id,
            "Partial",
            "Reasoned partly",
            "interrupted",
            None,
            None,
        )
        .await
        .unwrap();
    let interrupted = store.get(alice, chat).await.unwrap();
    assert_eq!(interrupted.messages[1].content, "Partial");
    assert_eq!(interrupted.messages[1].reasoning_content, "Reasoned partly");
    assert_eq!(interrupted.messages[1].status, "interrupted");
    assert!(interrupted.active_generation_id.is_none());
    let restored = store.select_version(alice, chat, first).await.unwrap();
    assert_eq!(restored.messages[1].content, "Saved answer");
    sqlx::query("INSERT INTO user_limit_overrides(user_id,limits)VALUES($1,$2)")
        .bind(alice)
        .bind(json!({"dailyRequests":1}))
        .execute(&pool)
        .await
        .unwrap();
    let failed = store
        .start_variant_turn(
            alice,
            chat,
            "Prompt",
            "fixture",
            "",
            0.7,
            256,
            None,
            1.0,
            "fixture",
            &[],
            false,
            first,
            "regenerate",
        )
        .await
        .unwrap_err();
    assert_eq!(failed.0, StatusCode::TOO_MANY_REQUESTS);
    let after = store.get(alice, chat).await.unwrap();
    assert_eq!(after.messages[1].id, first);
    assert_eq!(after.messages[1].versions.len(), 2);
    assert_eq!(
        store
            .finish_turn(
                alice,
                chat,
                pending.generation_id,
                "Stale",
                "completed",
                None,
                None
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn stored_alternatives_share_the_total_storage_budget(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool);
    let chat = store
        .create(owner, "Bounded versions", "fixture")
        .await
        .unwrap()
        .id;
    let first = store
        .start_turn(owner, chat, "A question", "fixture", "", 0.7, 256)
        .await
        .unwrap();
    let reasoning = "r".repeat(MAX_RESPONSE_BYTES - 1);
    store
        .finish_turn_with_reasoning(
            owner,
            chat,
            first.generation_id,
            "a",
            &reasoning,
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    let mut selected = first.assistant_id;
    for _ in 0..2 {
        let next = variant(&store, owner, chat, selected, "regenerate", "A question").await;
        store
            .finish_turn_with_reasoning(
                owner,
                chat,
                next.generation_id,
                "a",
                &reasoning,
                "completed",
                None,
                None,
            )
            .await
            .unwrap();
        selected = next.assistant_id;
    }
    assert_eq!(
        store
            .start_variant_turn(
                owner,
                chat,
                "A question",
                "fixture",
                "",
                0.7,
                256,
                None,
                1.0,
                "fixture",
                &[],
                false,
                selected,
                "regenerate"
            )
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        store.get(owner, chat).await.unwrap().messages[1]
            .versions
            .len(),
        3
    );
}

#[sqlx::test(migrations = false)]
async fn migration_preserves_existing_linear_message_ids_and_selected_history(pool: PgPool) {
    for migration in MIGRATOR.iter().filter(|m| m.version <= 9) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let owner = user(&pool).await;
    let chat = Uuid::new_v4();
    let prompt = Uuid::new_v4();
    let answer = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversations(id,owner_id,title,model)VALUES($1,$2,'Existing','fixture')",
    )
    .bind(chat)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status)VALUES($1,$2,0,'user','Original prompt','completed')").bind(prompt).bind(chat).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO messages(id,conversation_id,ordinal,role,content,status,generation_id)VALUES($1,$2,1,'assistant','Original answer','completed',$3)").bind(answer).bind(chat).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    for migration in MIGRATOR.iter().filter(|m| m.version >= 10) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let restored = Store::new(pool).get(owner, chat).await.unwrap();
    assert_eq!(
        restored.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![prompt, answer]
    );
    assert_eq!(restored.messages[1].content, "Original answer");
    assert_eq!(restored.messages[1].versions.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn variant_context_reads_selected_path_snapshots_and_initializes_imports(pool: PgPool) {
    let owner = user(&pool).await;
    let bob = user(&pool).await;
    let store = Store::new(pool.clone());
    let chat = store.create(owner, "Tuned", "fixture").await.unwrap().id;
    let mut answers = vec![];
    for (prompt, model, temperature) in [("First", "model-a", 0.3), ("Second", "model-b", 1.1)] {
        let t = store
            .start_turn_configured(
                owner,
                chat,
                prompt,
                model,
                "Be brief",
                temperature,
                128,
                Some(ReasoningEffort::Low),
                0.9,
                model,
            )
            .await
            .unwrap();
        store
            .finish_turn(
                owner,
                chat,
                t.generation_id,
                "Answer",
                "completed",
                None,
                None,
            )
            .await
            .unwrap();
        answers.push(t.assistant_id);
    }
    let saved = store.saved_settings(owner, chat).await.unwrap();
    assert_eq!(saved["temperature"], json!(1.1));
    assert_eq!(
        store.saved_settings(bob, chat).await.unwrap_err().0,
        StatusCode::NOT_FOUND
    );
    let prompts = store
        .get(owner, chat)
        .await
        .unwrap()
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.id)
        .collect::<Vec<_>>();
    // Each variant reuses the snapshot of the reply it replaces, not the latest settings.
    let edit = store
        .variant_context(owner, chat, prompts[0], "edit")
        .await
        .unwrap();
    assert_eq!(
        (edit.draft.as_str(), edit.model.as_str()),
        ("First", "model-a")
    );
    assert_eq!(edit.settings["temperature"], json!(0.3));
    assert_eq!(edit.settings["systemPrompt"], "Be brief");
    assert!(edit.settings.get("model").is_none());
    let regen = store
        .variant_context(owner, chat, answers[1], "regenerate")
        .await
        .unwrap();
    assert_eq!(
        (regen.draft.as_str(), regen.model.as_str()),
        ("Second", "model-b")
    );
    assert_eq!(regen.settings["temperature"], json!(1.1));
    for (source, mode) in [(prompts[1], "regenerate"), (answers[0], "edit")] {
        assert_eq!(
            store
                .variant_context(owner, chat, source, mode)
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let replaced = variant(&store, owner, chat, answers[0], "regenerate", "").await;
    store
        .finish_turn(
            owner,
            chat,
            replaced.generation_id,
            "Again",
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    // Versions that left the selected path cannot seed a variant.
    for (source, mode) in [(answers[0], "regenerate"), (prompts[1], "edit")] {
        assert_eq!(
            store
                .variant_context(owner, chat, source, mode)
                .await
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let current = store
        .variant_context(owner, chat, replaced.assistant_id, "regenerate")
        .await
        .unwrap();
    assert_eq!(
        (current.draft.as_str(), current.model.as_str()),
        ("First", "fixture")
    );
    assert_eq!(current.settings["temperature"], json!(0.7));
    // Imports initialize their path lazily; a variant preview may be the first read.
    let archive = store.export(owner, &[chat]).await.unwrap();
    let copy = store.import(owner, archive).await.unwrap()[0].id;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM messages WHERE conversation_id=$1 ORDER BY ordinal")
            .bind(copy)
            .fetch_all(&pool)
            .await
            .unwrap();
    let uninitialized: bool =
        sqlx::query_scalar("SELECT selected_leaf_id IS NULL FROM conversations WHERE id=$1")
            .bind(copy)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(uninitialized);
    let imported = store
        .variant_context(owner, copy, ids[1], "regenerate")
        .await
        .unwrap();
    assert_eq!(imported.draft, "First");
    assert_eq!(imported.settings["temperature"], json!(0.7));
}
