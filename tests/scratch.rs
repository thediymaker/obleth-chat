//! The scratch pad: each person sees and changes only their own jottings, even in a
//! shared project; a jotting's project is one they may use; the pad holds at most
//! 500 and refuses more; the list pages newest first. Requires a disposable
//! DATABASE_URL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    scratch, AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:8085";
struct Session {
    id: Uuid,
    cookie: String,
    csrf: String,
}
async fn session(pool: &PgPool, name: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,$3,'user')")
        .bind(id)
        .bind(format!("test_{}", id.simple()))
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let token = URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()));
    sqlx::query("INSERT INTO auth_sessions(token_hash,user_id,expires_at)VALUES($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(pool).await.unwrap();
    let csrf = URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!("obleth-chat-csrf:{token}").as_bytes(),
    ));
    Session {
        id,
        cookie: format!("obleth_chat_session={token}"),
        csrf,
    }
}
async fn fixture(pool: &PgPool) -> Router {
    let auth = Auth::new(
        pool.clone(),
        AuthConfig {
            origin: ORIGIN.into(),
            session_hours: 12,
            bootstrap_username: None,
            bootstrap_password: None,
            oidc: None,
            login_rate_per_minute: 1000,
            ..AuthConfig::default()
        },
    )
    .await
    .unwrap();
    scratch::router(AppState::new(auth, pool.clone(), None, 4))
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", ORIGIN)
        .header("cookie", &user.cookie)
        .header("x-csrf-token", &user.csrf)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let result = app.clone().oneshot(request).await.unwrap();
    let status = result.status();
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
/// A project of `owner`'s, shared with a new group that `members` are in.
async fn shared_project(pool: &PgPool, owner: Uuid, members: &[Uuid]) -> (Uuid, Uuid) {
    let group = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups(id,name) VALUES($1,$2)")
        .bind(group)
        .bind(format!("Lab {}", group.simple()))
        .execute(pool)
        .await
        .unwrap();
    for member in members {
        sqlx::query("INSERT INTO user_group_members(group_id,user_id) VALUES($1,$2)")
            .bind(group)
            .bind(member)
            .execute(pool)
            .await
            .unwrap();
    }
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,owner_id,name,scope,group_id) VALUES($1,$2,'Batching paper','group',$3)")
        .bind(project)
        .bind(owner)
        .bind(group)
        .execute(pool)
        .await
        .unwrap();
    (project, group)
}
fn jotting(id: &Value) -> String {
    format!("/api/scratch/{}", id.as_str().unwrap())
}
async fn jot(app: &Router, user: &Session, text: &str, project: Option<Uuid>) -> Value {
    let (status, body) = call(
        app,
        user,
        "POST",
        "/api/scratch",
        json!({"text": text, "projectId": project}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["jotting"].clone()
}
/// The texts on `user`'s pad, newest first.
async fn texts(app: &Router, user: &Session, query: &str) -> (Vec<String>, bool) {
    let (status, list) = call(
        app,
        user,
        "GET",
        &format!("/api/scratch{query}"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["text"].as_str().unwrap().to_owned())
        .collect();
    (items, list["more"].as_bool().unwrap())
}

#[sqlx::test(migrations = "./migrations")]
async fn jottings_are_their_owners_alone_even_in_a_shared_project(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let ben = session(&pool, "Ben").await;
    let (project, _) = shared_project(&pool, ana.id, &[ben.id]).await;
    let mine = jot(
        &app,
        &ana,
        "  Ask Maya about the GPU memory logs \n",
        Some(project),
    )
    .await;
    assert_eq!(mine["text"], "Ask Maya about the GPU memory logs");
    assert_eq!(mine["projectId"], json!(project));
    assert_eq!(mine["projectName"], "Batching paper");
    jot(&app, &ben, "Ben's own idea", Some(project)).await;

    // Ben, in the project's group, sees only his own; Ana's are hidden from him.
    assert_eq!(
        texts(&app, &ben, "").await,
        (vec!["Ben's own idea".to_owned()], false)
    );
    for (method, body) in [
        ("PATCH", json!({"text": "Changed by Ben"})),
        ("DELETE", json!(null)),
    ] {
        let (status, _) = call(&app, &ben, method, &jotting(&mine["id"]), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    assert_eq!(
        texts(&app, &ana, "").await.0,
        vec!["Ask Maya about the GPU memory logs".to_owned()]
    );

    // Ana changes and deletes her own.
    let (status, changed) = call(
        &app,
        &ana,
        "PATCH",
        &jotting(&mine["id"]),
        json!({"text": "Ask Maya about memory", "projectId": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["jotting"]["text"], "Ask Maya about memory");
    assert_eq!(changed["jotting"]["projectId"], Value::Null);
    // A change without projectId keeps the project as it is.
    let (_, kept) = call(
        &app,
        &ana,
        "PATCH",
        &jotting(&mine["id"]),
        json!({"projectId": project}),
    )
    .await;
    let (_, kept) = call(
        &app,
        &ana,
        "PATCH",
        &jotting(&kept["jotting"]["id"]),
        json!({"text": "Ask Maya today"}),
    )
    .await;
    assert_eq!(kept["jotting"]["projectId"], json!(project));
    let (status, _) = call(&app, &ana, "DELETE", &jotting(&mine["id"]), json!(null)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, &ana, "DELETE", &jotting(&mine["id"]), json!(null)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(texts(&app, &ana, "").await, (vec![], false));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_jottings_project_is_one_the_person_may_use(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let ben = session(&pool, "Ben").await;
    let cy = session(&pool, "Cy").await;
    let (project, group) = shared_project(&pool, ana.id, &[ben.id]).await;

    // Cy is not in the group: the project is not found for him, to make or to move into.
    let (status, _) = call(
        &app,
        &cy,
        "POST",
        "/api/scratch",
        json!({"text": "Into Ana's project", "projectId": project}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let cys = jot(&app, &cy, "Mine", None).await;
    let (status, _) = call(
        &app,
        &cy,
        "PATCH",
        &jotting(&cys["id"]),
        json!({"projectId": project}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        &cy,
        "PATCH",
        &jotting(&cys["id"]),
        json!({"projectId": Uuid::new_v4()}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Ben, a member, tags his jotting with the project. Once he leaves the group the
    // jotting stays his, without the project's name.
    let bens = jot(&app, &ben, "For the paper", Some(project)).await;
    sqlx::query("DELETE FROM user_group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(ben.id)
        .execute(&pool)
        .await
        .unwrap();
    let (_, list) = call(&app, &ben, "GET", "/api/scratch", json!(null)).await;
    assert_eq!(list["items"][0]["id"], bens["id"]);
    assert_eq!(list["items"][0]["projectId"], Value::Null);
    assert_eq!(list["items"][0]["projectName"], Value::Null);

    // Deleting a project leaves its jottings on the pad, in no project.
    let anas = jot(&app, &ana, "Check the knee", Some(project)).await;
    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let (_, list) = call(&app, &ana, "GET", "/api/scratch", json!(null)).await;
    assert_eq!(list["items"][0]["id"], anas["id"]);
    assert_eq!(list["items"][0]["projectId"], Value::Null);
}

#[sqlx::test(migrations = "./migrations")]
async fn jottings_keep_to_their_limits(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    for (body, why) in [
        (json!({"text": "   "}), "blank text"),
        (json!({"text": "x".repeat(4001)}), "long text"),
        (json!({"text": "Fine", "pinned": true}), "an unknown field"),
    ] {
        let (status, _) = call(&app, &ana, "POST", "/api/scratch", body).await;
        assert!(status.is_client_error(), "{why}: {status}");
    }
    // 4,000 characters fit, counted as characters, not bytes.
    jot(&app, &ana, &"é".repeat(4000), None).await;

    // 499 more fill the pad; the next is refused with a reason, and nothing is dropped.
    sqlx::query(
        "INSERT INTO scratch_jottings (id, owner_id, text, created_at)
         SELECT gen_random_uuid(), $1, 'Old ' || n, NOW() - n * INTERVAL '1 minute' FROM generate_series(1, 498) n",
    )
    .bind(ana.id)
    .execute(&pool)
    .await
    .unwrap();
    jot(&app, &ana, "The 500th", None).await;
    let (status, refused) = call(
        &app,
        &ana,
        "POST",
        "/api/scratch",
        json!({"text": "One too many"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("500 jottings"),
        "{refused}"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM scratch_jottings WHERE owner_id = $1")
            .bind(ana.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 500);
    // Someone else's pad is not affected by Ana's.
    let ben = session(&pool, "Ben").await;
    jot(&app, &ben, "Room here", None).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn the_pad_pages_newest_first(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    for n in 1..=5 {
        jot(&app, &ana, &format!("Jotting {n}"), None).await;
    }
    assert_eq!(
        texts(&app, &ana, "?limit=2").await,
        (vec!["Jotting 5".to_owned(), "Jotting 4".to_owned()], true)
    );
    assert_eq!(
        texts(&app, &ana, "?offset=2&limit=2").await,
        (vec!["Jotting 3".to_owned(), "Jotting 2".to_owned()], true)
    );
    assert_eq!(
        texts(&app, &ana, "?offset=4&limit=2").await,
        (vec!["Jotting 1".to_owned()], false)
    );
    assert_eq!(texts(&app, &ana, "").await.0.len(), 5);
}
