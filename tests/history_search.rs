//! Regressions for PostgreSQL's 1 MiB tsvector limit and complete-message search.
use obleth_chat::store::{HistoryQuery, Store, MAX_RESPONSE_BYTES};
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name)VALUES($1,$2,'Search test')")
        .bind(id)
        .bind(format!("search-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}
async fn answer(store: &Store, owner: Uuid, content: &str) -> (Uuid, Uuid) {
    let conversation = store.create(owner, "Untitled", "fixture").await.unwrap();
    let turn = store
        .start_turn(
            owner,
            conversation.id,
            "Save this response",
            "fixture",
            "",
            0.7,
            2048,
        )
        .await
        .unwrap();
    store
        .finish_turn(
            owner,
            conversation.id,
            turn.generation_id,
            content,
            "completed",
            None,
            None,
        )
        .await
        .unwrap();
    (conversation.id, turn.assistant_id)
}
async fn matches(store: &Store, owner: Uuid, query: &str) -> Vec<Uuid> {
    store
        .history(
            owner,
            HistoryQuery {
                query: Some(query.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .conversations
        .into_iter()
        .map(|c| c.id)
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn near_limit_distinct_word_response_persists_and_tail_is_searchable(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let store = Store::new(pool.clone());
    let mut content = (0..100_000)
        .map(|i| format!("word{i} "))
        .collect::<String>();
    content.push_str("tailneedle");
    assert!(content.len() > 980_000 && content.len() < MAX_RESPONSE_BYTES);
    // This reproduces the former index failure while remaining a legal message.
    let old = sqlx::query_scalar::<_, String>("SELECT to_tsvector('simple',$1)::text")
        .bind(&content)
        .fetch_one(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        old.as_database_error().unwrap().code().as_deref(),
        Some("54000")
    );
    let (chat, message) = answer(&store, alice, &content).await;
    assert_eq!(
        store.get(alice, chat).await.unwrap().messages[1].content,
        content
    );
    assert_eq!(matches(&store, alice, "tailneedle").await, vec![chat]);
    assert_eq!(matches(&store, alice, "word0 tailneedle").await, vec![chat]);
    assert!(matches(&store, alice, "word0 -tailneedle").await.is_empty());
    assert!(matches(&store, bob, "tailneedle").await.is_empty());
    let (count,largest):(i64,i32)=sqlx::query_as("SELECT COUNT(*),MAX(pg_column_size(document)) FROM message_search_chunks WHERE message_id=$1")
        .bind(message).fetch_one(&pool).await.unwrap();
    assert!(
        count > 100,
        "all content must be indexed, not just the first chunk"
    );
    assert!(largest < 1024 * 1024);
    let archive = store.export(alice, &[chat]).await.unwrap();
    let imported = store.import(bob, archive).await.unwrap();
    assert_eq!(
        matches(&store, bob, "tailneedle").await,
        vec![imported[0].id]
    );
    store.delete(alice, chat).await.unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_search_chunks WHERE message_id=$1")
            .bind(message)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "message deletion must cascade into its search index"
    );
    assert_eq!(
        matches(&store, bob, "tailneedle").await,
        vec![imported[0].id]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn chunk_boundaries_boolean_search_and_content_updates_preserve_results(pool: PgPool) {
    let owner = user(&pool).await;
    let store = Store::new(pool.clone());
    let content = format!(
        "{}crossboundary constellation {}distantmarker",
        "padding ".repeat(1022),
        "padding ".repeat(4096)
    );
    let (chat, message) = answer(&store, owner, &content).await;
    assert_eq!(
        matches(&store, owner, "\"crossboundary constellation\"").await,
        vec![chat]
    );
    assert_eq!(
        matches(&store, owner, "crossboundary distantmarker").await,
        vec![chat]
    );
    assert_eq!(
        matches(&store, owner, "absent OR distantmarker").await,
        vec![chat]
    );
    assert!(matches(&store, owner, "crossboundary -distantmarker")
        .await
        .is_empty());
    sqlx::query("UPDATE messages SET content=$2 WHERE id=$1")
        .bind(message)
        .bind("Revised éclairage entry with :: punctuation")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches(&store, owner, "distantmarker").await.is_empty());
    assert_eq!(matches(&store, owner, "éclairage").await, vec![chat]);
    assert_eq!(matches(&store, owner, "::").await, vec![chat]);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_search_chunks WHERE message_id=$1")
            .bind(message)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "updates must remove obsolete chunks");
}

#[sqlx::test(migrations = "./migrations")]
async fn negation_punctuation_titles_and_page_totals_match_per_message(pool: PgPool) {
    let owner = user(&pool).await;
    let other = user(&pool).await;
    let store = Store::new(pool.clone());
    let (red, _) = answer(&store, owner, "red green").await;
    let (blue, _) = answer(&store, owner, "blue -> arrow").await;
    let (empty, _) = answer(&store, owner, "").await;
    answer(&store, other, "red green").await;
    let titled = store
        .create(owner, "Red notes", "fixture")
        .await
        .unwrap()
        .id;
    let sorted = |mut ids: Vec<Uuid>| {
        ids.sort();
        ids
    };
    assert_eq!(
        sorted(matches(&store, owner, "red").await),
        sorted(vec![red, titled])
    );
    assert_eq!(matches(&store, owner, "red -blue").await, vec![red]);
    assert!(matches(&store, owner, "red -green").await.is_empty());
    // Pure negation matches any message without the words, including empty ones.
    assert_eq!(
        sorted(matches(&store, owner, "-red").await),
        sorted(vec![red, blue, empty])
    );
    assert_eq!(
        sorted(matches(&store, owner, "-save -red").await),
        sorted(vec![blue, empty])
    );
    assert_eq!(matches(&store, owner, "->").await, vec![blue]);
    assert_eq!(
        matches(&store, owner, "green blue").await,
        Vec::<Uuid>::new()
    );
    let page = |offset| HistoryQuery {
        query: Some("-red".into()),
        limit: Some(2),
        offset: Some(offset),
        ..Default::default()
    };
    let first = store.history(owner, page(0)).await.unwrap();
    assert_eq!((first.conversations.len(), first.total), (2, 3));
    assert!(first.has_more);
    let past = store.history(owner, page(10)).await.unwrap();
    assert_eq!((past.conversations.len(), past.total), (0, 3));
    assert!(!past.has_more);
}
