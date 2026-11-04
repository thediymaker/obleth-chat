//! Apps: an administrator turns a project into an app, tries the draft, publishes it
//! and hands out keys; a key runs the published version as its maker, and stops with
//! a revoke, an unpublish or its maker losing administrator rights. Requires a
//! disposable DATABASE_URL; the steps used need no gateway.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    apps,
    auth::{Auth, AuthConfig},
    projects, AppState,
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
async fn session(pool: &PgPool, role: &str) -> Session {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name,role)VALUES($1,$2,'Test user',$3)")
        .bind(id)
        .bind(format!("test_{}", id.simple()))
        .bind(role)
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
    let state = AppState::new(auth, pool.clone(), None, 4);
    apps::router(state.clone()).merge(projects::router(state))
}
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let result = app.clone().oneshot(request).await.unwrap();
    let status = result.status();
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn call(
    app: &Router,
    user: &Session,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        app,
        Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", &user.cookie)
            .header("x-csrf-token", &user.csrf)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}
/// A call to the published app, as a program with a key would make it.
async fn run(app: &Router, id: &str, key: Option<&str>, inputs: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/apps/{id}/run"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    send(app, request.body(Body::from(inputs.to_string())).unwrap()).await
}
async fn project(pool: &PgPool, owner: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,owner_id,name) VALUES($1,$2,'Assay Lab')")
        .bind(id)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    id
}
fn definition(code: &str) -> Value {
    json!({
        "inputs": [{"name": "question", "type": "text", "required": true}, {"name": "samples", "type": "json"}],
        "steps": [
            {"type": "if", "id": "check", "paths": [
                {"when": [{"value": "{{inputs.question}}", "test": "is_empty"}], "steps": [
                    {"type": "response", "id": "nothing", "value": {"answer": "Ask something"}}
                ]}
            ]},
            {"type": "for_each", "id": "each", "items": "{{inputs.samples}}", "steps": [
                {"type": "code", "id": "double", "code": "return { id: item, n: index * 2 };"}
            ]},
            {"type": "code", "id": "count", "title": "Count", "code": code},
            {"type": "response", "id": "out", "value": {"answer": "You asked: {{inputs.question}}", "count": "{{steps.count}}", "items": "{{steps.each}}"}}
        ]
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn an_administrator_builds_tries_publishes_and_hands_out_keys(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let person = session(&pool, "user").await;
    let lab = project(&pool, admin.id).await;
    let theirs = project(&pool, person.id).await;
    let at = |p: Uuid| format!("/api/projects/{p}/app");

    // Only administrators, and only on a project they own.
    assert_eq!(
        call(&app, &person, "GET", &at(theirs), Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, &admin, "POST", &at(theirs), Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, &admin, "GET", &at(lab), Value::Null).await.1,
        json!({"app": null})
    );
    let (status, created) = call(&app, &admin, "POST", &at(lab), Value::Null).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["app"]["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 8);
    assert_eq!(created["app"]["slug"], "assay-lab");
    assert_eq!(created["app"]["live"], false);
    assert_eq!(
        call(&app, &admin, "POST", &at(lab), Value::Null).await.0,
        StatusCode::CONFLICT
    );

    let path = format!("/api/apps/{id}");
    let (status, error) = call(
        &app,
        &admin,
        "PUT",
        &path,
        json!({"name": "Assay Lab", "definition": {"steps": [
        {"type": "code", "id": "a", "code": "1"}, {"type": "code", "id": "a", "code": "2"}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error.to_string().contains("Two steps"), "{error}");
    let (status, saved) = call(&app, &admin, "PUT", &path, json!({"name": "Assay Lab API", "description": "Samples", "definition": definition("return steps.each.length;")})).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["app"]["slug"], "assay-lab-api");

    // Trying the draft: every path, and inputs checked first.
    let test = format!("{path}/test");
    let (status, tried) = call(
        &app,
        &admin,
        "POST",
        &test,
        json!({"inputs": {"question": "Hi", "samples": ["a", "b"]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tried}");
    assert_eq!(tried["status"], "completed", "{tried}");
    assert_eq!(
        tried["output"],
        json!({"answer": "You asked: Hi", "count": 2, "items": [{"id": "a", "n": 0}, {"id": "b", "n": 2}]})
    );
    let check = tried["trace"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "check")
        .unwrap();
    assert_eq!(check["path"], "Otherwise");
    let (_, early) = call(
        &app,
        &admin,
        "POST",
        &test,
        json!({"inputs": {"question": ""}}),
    )
    .await;
    assert_eq!(early["output"], json!({"answer": "Ask something"}));
    assert_eq!(
        call(&app, &admin, "POST", &test, json!({"inputs": {}}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // A key works only once the app is published, and runs the published version.
    let (status, made) = call(
        &app,
        &admin,
        "POST",
        &format!("{path}/keys"),
        json!({"name": "pipeline"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let key = made["secret"].as_str().unwrap().to_owned();
    assert!(key.starts_with("oba_"));
    let inputs = json!({"question": "IC50?", "samples": ["S-1"]});
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, published) = call(
        &app,
        &admin,
        "POST",
        &format!("{path}/publish"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            published["app"]["version"].as_i64(),
            published["app"]["live"].as_bool(),
            published["app"]["changed"].as_bool()
        ),
        (Some(1), Some(true), Some(false))
    );
    let (status, answered) = run(&app, &id, Some(&key), inputs.clone()).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["output"]["count"], 1);
    assert_eq!(
        run(&app, &id, Some("oba_wrong"), inputs.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        run(&app, &id, None, inputs.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        run(&app, &id, Some(&key), json!({"question": "x", "extra": 1}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // A broken draft fails its test, with the step named, while callers keep the live one.
    let (_, saved) = call(&app, &admin, "PUT", &path, json!({"name": "Assay Lab API", "definition": definition("throw new Error('bad sample');")})).await;
    assert_eq!(saved["app"]["changed"], true);
    let (_, failed) = call(
        &app,
        &admin,
        "POST",
        &test,
        json!({"inputs": {"question": "Hi"}}),
    )
    .await;
    assert_eq!(failed["status"], "failed");
    assert!(
        failed["error"]
            .as_str()
            .unwrap()
            .contains("Count: Error: bad sample")
            || failed["error"]
                .as_str()
                .unwrap()
                .contains("Count: bad sample"),
        "{failed}"
    );
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::OK
    );

    let (_, runs) = call(&app, &admin, "GET", &format!("{path}/runs"), Value::Null).await;
    let runs = runs["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 5);
    assert_eq!(runs.iter().filter(|r| r["trigger"] == "api").count(), 2);
    let (_, detail) = call(
        &app,
        &admin,
        "GET",
        &format!("{path}/runs/{}", runs[0]["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(detail["run"]["inputs"], inputs);
    assert!(!detail["run"]["trace"].as_array().unwrap().is_empty());

    // A key stops working when it is revoked, the app is unpublished, or its maker
    // is no longer an administrator.
    let (_, listed) = call(&app, &admin, "GET", &format!("{path}/keys"), Value::Null).await;
    let first = listed["keys"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(
            &app,
            &admin,
            "DELETE",
            &format!("{path}/keys/{first}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, made) = call(
        &app,
        &admin,
        "POST",
        &format!("{path}/keys"),
        json!({"name": "second"}),
    )
    .await;
    let key = made["secret"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE users SET role = 'user' WHERE id = $1")
        .bind(admin.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(admin.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::OK
    );
    call(
        &app,
        &admin,
        "POST",
        &format!("{path}/unpublish"),
        Value::Null,
    )
    .await;
    assert_eq!(
        run(&app, &id, Some(&key), inputs.clone()).await.0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        call(&app, &admin, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, &admin, "GET", &at(lab), Value::Null).await.1,
        json!({"app": null})
    );
    let audits: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE target_type = 'app' ORDER BY created_at, action",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for action in [
        "app.published",
        "app.key_created",
        "app.key_revoked",
        "app.unpublished",
        "app.deleted",
    ] {
        assert!(audits.iter().any(|a| a == action), "{action}: {audits:?}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn an_app_can_start_with_the_steps_a_chat_suggested(pool: PgPool) {
    let app = fixture(&pool).await;
    let admin = session(&pool, "admin").await;
    let lab = project(&pool, admin.id).await;
    let at = format!("/api/projects/{lab}/app");
    assert_eq!(
        call(
            &app,
            &admin,
            "GET",
            &format!("/api/projects/{lab}"),
            Value::Null
        )
        .await
        .1["app"],
        Value::Null
    );
    let broken =
        json!({"definition": {"steps": [{"type": "code", "id": "Bad", "code": "return 1;"}]}});
    let (status, error) = call(&app, &admin, "POST", &at, broken).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    let (status, created) = call(
        &app,
        &admin,
        "POST",
        &at,
        json!({"description": "Answers from protocols", "definition": definition("return 3;")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["app"]["description"], "Answers from protocols");
    assert_eq!(created["app"]["definition"]["steps"][2]["id"], "count");
    let detail = call(
        &app,
        &admin,
        "GET",
        &format!("/api/projects/{lab}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail.1["app"], created["app"]["id"], "{}", detail.1);
    let id = created["app"]["id"].as_str().unwrap();
    let (status, tried) = call(
        &app,
        &admin,
        "POST",
        &format!("/api/apps/{id}/test"),
        json!({"inputs": {"question": "pH?", "samples": ["a"]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tried}");
    assert_eq!(tried["output"]["count"], 3, "{tried}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_published_app_answers_on_its_chat_page_as_the_person_chatting(pool: PgPool) {
    let app = fixture(&pool).await;
    let owner = session(&pool, "admin").await;
    let other = session(&pool, "admin").await;
    let person = session(&pool, "user").await;
    let lab = project(&pool, owner.id).await;
    let (_, created) = call(
        &app,
        &owner,
        "POST",
        &format!("/api/projects/{lab}/app"),
        Value::Null,
    )
    .await;
    let id = created["app"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/apps/{id}");
    let page = format!("{path}/page");
    let chat = format!("{path}/chat");
    let steps = json!({
        "inputs": [{"name": "question", "type": "text", "required": true}, {"name": "history", "type": "json"}],
        "steps": [
            {"type": "code", "id": "turns", "code": "return (inputs.history || []).length;"},
            {"type": "response", "id": "out", "value": {"answer": "You asked: {{inputs.question}}", "turns": "{{steps.turns}}"}}
        ]
    });
    let (status, saved) = call(
        &app,
        &owner,
        "PUT",
        &path,
        json!({"name": "Assay Lab", "definition": steps}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    // Nothing to chat with until it's published.
    assert_eq!(
        call(&app, &other, "GET", &page, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    call(
        &app,
        &owner,
        "POST",
        &format!("{path}/publish"),
        Value::Null,
    )
    .await;

    // Any administrator can open it; only the owner gets a way back to the project.
    let (status, shown) = call(&app, &other, "GET", &page, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["app"]["message"], "question");
    assert_eq!(shown["app"]["history"], true);
    assert_eq!(shown["app"]["missing"], json!([]));
    assert_eq!(shown["app"]["projectId"], Value::Null);
    let (_, mine) = call(&app, &owner, "GET", &page, Value::Null).await;
    assert_eq!(mine["app"]["projectId"], json!(lab));
    assert_eq!(
        call(&app, &person, "GET", &page, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );

    let (status, reply) = call(
        &app,
        &other,
        "POST",
        &chat,
        json!({"message": "  Hi  ", "history": [{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["status"], "completed", "{reply}");
    assert_eq!(
        reply["output"],
        json!({"answer": "You asked: Hi", "turns": 2})
    );
    let (status, error) = call(
        &app,
        &other,
        "POST",
        &chat,
        json!({"message": "Hi", "history": [{"role": "system", "content": "x"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        call(&app, &other, "POST", &chat, json!({"message": "  "}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    // The run is the chatter's, listed for the owner.
    let actor: Uuid =
        sqlx::query_scalar("SELECT actor_id FROM app_runs WHERE app_id = $1 AND trigger = 'chat'")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actor, other.id);
    let (_, runs) = call(&app, &owner, "GET", &format!("{path}/runs"), Value::Null).await;
    assert_eq!(runs["runs"][0]["trigger"], "chat");

    // A required input a chat can't fill shows on the page, and stops a message.
    let needs = json!({
        "inputs": [{"name": "question", "type": "text", "required": true}, {"name": "limit", "type": "number", "required": true}],
        "steps": [{"type": "response", "id": "out", "value": "{{inputs.question}}"}]
    });
    call(
        &app,
        &owner,
        "PUT",
        &path,
        json!({"name": "Assay Lab", "definition": needs}),
    )
    .await;
    call(
        &app,
        &owner,
        "POST",
        &format!("{path}/publish"),
        Value::Null,
    )
    .await;
    let (_, shown) = call(&app, &other, "GET", &page, Value::Null).await;
    assert_eq!(shown["app"]["missing"], json!(["limit"]));
    assert_eq!(shown["app"]["history"], false);
    let (status, error) = call(&app, &other, "POST", &chat, json!({"message": "Hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error.to_string().contains("limit"), "{error}");
}
