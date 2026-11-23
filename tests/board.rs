//! A project's board: whoever may use the project sees and changes its cards, cards
//! keep to their limits, a card moves from an idea to done with its history, and two
//! people moving cards at once leave every column in one order. Requires a
//! disposable DATABASE_URL.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use obleth_chat::{
    auth::{Auth, AuthConfig},
    board, AppState,
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
    board::router(AppState::new(auth, pool.clone(), None, 4))
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
async fn chat(pool: &PgPool, owner: Uuid, project: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO conversations (id, owner_id, title, model, project_id) VALUES ($1, $2, 'Decode sweep', 'm', $3)")
        .bind(id)
        .bind(owner)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    id
}
fn cards(project: Uuid) -> String {
    format!("/api/projects/{project}/cards")
}
fn card(project: Uuid, id: &Value) -> String {
    format!("/api/projects/{project}/cards/{}", id.as_str().unwrap())
}
async fn add(app: &Router, user: &Session, project: Uuid, title: &str, status: &str) -> Value {
    let (status_code, body) = call(
        app,
        user,
        "POST",
        &cards(project),
        json!({"title": title, "status": status}),
    )
    .await;
    assert_eq!(status_code, StatusCode::CREATED, "{body}");
    body["card"]["id"].clone()
}
/// The titles in a column, in order, from the board as `user` sees it.
async fn column(app: &Router, user: &Session, project: Uuid, status: &str) -> Vec<String> {
    let (_, board) = call(app, user, "GET", &cards(project), json!(null)).await;
    board["cards"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] == status)
        .map(|c| c["title"].as_str().unwrap().to_owned())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn group_members_share_a_board_and_others_do_not_see_it(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let ben = session(&pool, "Ben").await;
    let cy = session(&pool, "Cy").await;
    let (project, group) = shared_project(&pool, ana.id, &[ben.id]).await;
    let id = add(&app, &ana, project, "Add a limitations paragraph", "idea").await;

    // Ben, in the group, sees Ana's card and moves it; Cy, outside it, sees nothing.
    let (status, board) = call(&app, &ben, "GET", &cards(project), json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(board["cards"][0]["title"], "Add a limitations paragraph");
    assert_eq!(board["cards"][0]["madeBy"], "person");
    assert_eq!(board["cards"][0]["maker"]["name"], "Ana");
    let (status, moved) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"status": "next"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["card"]["status"], "next");
    let ben_idea = add(&app, &ben, project, "A shorter title?", "idea").await;
    for (method, path, body) in [
        ("GET", cards(project), json!(null)),
        ("POST", cards(project), json!({"title": "Mine now"})),
        ("GET", card(project, &id), json!(null)),
        ("PATCH", card(project, &id), json!({"status": "done"})),
        ("DELETE", card(project, &id), json!(null)),
        (
            "PUT",
            format!("{}/order", cards(project)),
            json!({"status": "next", "ids": [id]}),
        ),
    ] {
        let (status, _) = call(&app, &cy, method, &path, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    // A card id from another project is not found through this one.
    let (other, _) = shared_project(&pool, cy.id, &[]).await;
    let (status, _) = call(
        &app,
        &cy,
        "PATCH",
        &card(other, &id),
        json!({"status": "done"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Leaving the group ends Ben's access; Ana deletes his idea.
    sqlx::query("DELETE FROM user_group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(ben.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call(&app, &ben, "GET", &cards(project), json!(null)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, &ana, "DELETE", &card(project, &ben_idea), json!(null)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        column(&app, &ana, project, "idea").await,
        Vec::<String>::new()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn cards_keep_to_their_limits(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let (project, _) = shared_project(&pool, ana.id, &[]).await;
    let id = add(&app, &ana, project, "Fix the formatting", "idea").await;
    let step = |n: usize| json!({"text": format!("Step {n}"), "state": "todo"});
    for (body, why) in [
        (json!({"title": ""}), "an empty title"),
        (json!({"title": "x".repeat(201)}), "a long title"),
        (
            json!({"title": "Fine", "notes": "x".repeat(4001)}),
            "long notes",
        ),
        (
            json!({"title": "Fine", "kind": "x".repeat(41)}),
            "a long kind",
        ),
        (
            json!({"title": "Fine", "status": "suggested"}),
            "a person suggesting",
        ),
        (
            json!({"title": "Fine", "status": "done"}),
            "a new card in Done",
        ),
        (
            json!({"title": "Fine", "steps": (0..13).map(step).collect::<Vec<_>>()}),
            "13 steps",
        ),
        (json!({"title": "Fine", "owner": "me"}), "an unknown field"),
    ] {
        let (status, _) = call(&app, &ana, "POST", &cards(project), body).await;
        assert!(status.is_client_error(), "{why}: {status}");
    }
    for (body, why) in [
        (
            json!({"steps": [{"text": "Go", "state": "maybe"}]}),
            "a step state",
        ),
        (
            json!({"steps": [{"text": "x".repeat(201), "state": "todo"}]}),
            "a long step",
        ),
        (json!({"progress": "x".repeat(201)}), "a long progress note"),
        (
            json!({"question": "Which?", "options": ["a", "b", "c", "d", "e"]}),
            "five options",
        ),
        (
            json!({"question": "Which?", "options": ["x".repeat(81)]}),
            "a long option",
        ),
        (json!({"status": "archived"}), "an unknown column"),
        (json!({"status": "suggested"}), "a person suggesting"),
        (json!({"worker": "someone"}), "an unknown worker"),
        (
            json!({"worker": "assistant"}),
            "the assistant without a chat",
        ),
        (
            json!({"links": (0..21).map(|n| json!({"kind": "job", "label": n.to_string(), "ref": ""})).collect::<Vec<_>>()}),
            "21 links",
        ),
    ] {
        let (status, _) = call(&app, &ana, "PATCH", &card(project, &id), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}");
    }
    let (status, body) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"steps": (0..12).map(step).collect::<Vec<_>>()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["card"]["steps"].as_array().unwrap().len(), 12);

    // A full board takes no more cards.
    sqlx::query(
        "INSERT INTO project_cards (id, project_id, title, made_by, status, position)
         SELECT gen_random_uuid(), $1, 'Done ' || n, 'person', 'done', n FROM generate_series(1, $2) n",
    )
    .bind(project)
    .bind((board::MAX_CARDS - 1) as i32)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(
        &app,
        &ana,
        "POST",
        &cards(project),
        json!({"title": "One more"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("1,000 cards"),
        "{body}"
    );
    // The board shows the latest few done; the rest page.
    let (_, board) = call(&app, &ana, "GET", &cards(project), json!(null)).await;
    assert_eq!(board["doneCount"], board::MAX_CARDS - 1);
    assert_eq!(
        board["cards"].as_array().unwrap().len(),
        1 + board::DONE_SHOWN as usize
    );
    let (_, page) = call(
        &app,
        &ana,
        "GET",
        &format!("{}?status=done&offset=990&limit=50", cards(project)),
        json!(null),
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 9);
    assert_eq!(page["more"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_card_goes_from_an_idea_to_done_and_keeps_its_history(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let ben = session(&pool, "Ben").await;
    let (project, _) = shared_project(&pool, ana.id, &[ben.id]).await;
    let id = add(&app, &ana, project, "Sweep max-num-seqs", "idea").await;
    let (_, moved) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"status": "next", "kind": "Experiment"}),
    )
    .await;
    assert_eq!(moved["card"]["kind"], "Experiment");

    // A chat outside the project, or someone else's, can't work it.
    let elsewhere = chat(&pool, ana.id, None).await;
    let (status, _) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"chatId": elsewhere}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let bens = chat(&pool, ben.id, Some(project)).await;
    let (status, _) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"chatId": bens}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Started with the assistant in Ana's chat: Now, the assistant on it.
    let chat_id = chat(&pool, ana.id, Some(project)).await;
    let (status, started) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"chatId": chat_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let c = &started["card"];
    assert_eq!(
        (c["status"].as_str(), c["worker"].as_str()),
        (Some("doing"), Some("assistant"))
    );
    assert_eq!(c["chat"]["id"], json!(chat_id));
    assert_eq!(c["chat"]["mine"], true);
    assert_eq!(c["chat"]["title"], "Decode sweep");
    assert_eq!(c["chat"]["working"], false);
    assert!(c["startedAt"].is_string());

    // While the chat writes a reply the card is alive, for Ben too, who sees no title.
    sqlx::query("UPDATE conversations SET active_generation = $2, generation_expires = NOW() + INTERVAL '1 minute' WHERE id = $1")
        .bind(chat_id)
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    let (_, seen) = call(&app, &ben, "GET", &card(project, &id), json!(null)).await;
    assert_eq!(seen["card"]["chat"]["working"], true);
    assert_eq!(seen["card"]["chat"]["mine"], false);
    assert_eq!(seen["card"]["chat"]["title"], Value::Null);

    // An answer only goes to a card that asks.
    let (status, _) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"answer": "Keep log"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, asked) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &id),
        json!({"question": "Keep the log axis?", "options": ["Keep log", "Linear", "Keep log"]}),
    )
    .await;
    assert_eq!(asked["card"]["status"], "waiting");
    assert_eq!(asked["card"]["options"], json!(["Keep log", "Linear"]));
    let (_, answered) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"answer": "Linear"}),
    )
    .await;
    let c = &answered["card"];
    assert_eq!(
        (c["status"].as_str(), c["answer"].as_str()),
        (Some("doing"), Some("Linear"))
    );
    assert_eq!(c["worker"], "assistant");

    // Ben takes it over, then marks it done.
    let (_, taken) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"worker": "me"}),
    )
    .await;
    assert_eq!(taken["card"]["worker"], "person");
    assert_eq!(taken["card"]["workerUser"]["name"], "Ben");
    let (_, done) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"status": "done"}),
    )
    .await;
    assert!(done["card"]["doneAt"].is_string());
    let (_, reopened) = call(
        &app,
        &ben,
        "PATCH",
        &card(project, &id),
        json!({"status": "next"}),
    )
    .await;
    assert_eq!(reopened["card"]["doneAt"], Value::Null);
    assert_eq!(reopened["card"]["worker"], Value::Null);

    let (_, open) = call(&app, &ana, "GET", &card(project, &id), json!(null)).await;
    let history: Vec<(String, String)> = open["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["action"].as_str().unwrap().to_owned(),
                e["actorName"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    let expected = [
        ("made", "Ana"),
        ("moved", "Ana"),
        ("picked", "Ana"),
        ("asked", "Ana"),
        ("answered", "Ben"),
        ("picked", "Ben"),
        ("done", "Ben"),
        ("moved", "Ben"),
    ];
    assert_eq!(
        history,
        expected
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect::<Vec<_>>()
    );
    assert_eq!(open["history"][4]["detail"], "Linear");
    assert_eq!(open["history"][1]["from"], "idea");
    assert_eq!(open["history"][1]["to"], "next");

    // Without a chat an answered card goes back to Up next; deleting its chat keeps the card.
    let other = add(&app, &ana, project, "Fig. 2 axis", "idea").await;
    call(
        &app,
        &ana,
        "PATCH",
        &card(project, &other),
        json!({"question": "Log or linear?"}),
    )
    .await;
    let (_, answered) = call(
        &app,
        &ana,
        "PATCH",
        &card(project, &other),
        json!({"answer": "Log"}),
    )
    .await;
    assert_eq!(answered["card"]["status"], "next");
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(chat_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, kept) = call(&app, &ana, "GET", &card(project, &id), json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kept["card"]["chat"], Value::Null);
}

#[sqlx::test(migrations = "./migrations")]
async fn up_next_keeps_its_order_when_two_people_move_cards_at_once(pool: PgPool) {
    let app = fixture(&pool).await;
    let ana = session(&pool, "Ana").await;
    let ben = session(&pool, "Ben").await;
    let (project, _) = shared_project(&pool, ana.id, &[ben.id]).await;
    let mut ids = vec![];
    for n in 0..8 {
        ids.push(add(&app, &ana, project, &format!("Card {n}"), "next").await);
    }
    assert_eq!(
        column(&app, &ana, project, "next").await,
        (0..8).map(|n| format!("Card {n}")).collect::<Vec<_>>()
    );
    // Moving up and down by position, and between columns, from two people at once.
    let mut tasks = vec![];
    for round in 0..24usize {
        let app = app.clone();
        let who = if round % 2 == 0 { &ana } else { &ben };
        let session = Session {
            id: who.id,
            cookie: who.cookie.clone(),
            csrf: who.csrf.clone(),
        };
        let id = ids[round % ids.len()].clone();
        let body = match round % 4 {
            0 => json!({"position": 0}),
            1 => json!({"position": 5}),
            2 => json!({"status": "idea"}),
            _ => json!({"status": "next", "position": 2}),
        };
        tasks.push(tokio::spawn(async move {
            let (status, body) = call(&app, &session, "PATCH", &card(project, &id), body).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    // Every card is on the board once, and no two in a column share a place.
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "SELECT status, position FROM project_cards WHERE project_id = $1 ORDER BY status, position",
    )
    .bind(project)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 8);
    let mut seen = std::collections::HashSet::new();
    for row in &rows {
        assert!(seen.insert(row.clone()), "two cards at {row:?}");
    }

    // A whole column put in order; cards left out keep theirs after the named ones.
    let next: Vec<Value> = {
        let (_, board) = call(&app, &ana, "GET", &cards(project), json!(null)).await;
        board["cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["status"] == "next")
            .map(|c| c["id"].clone())
            .collect()
    };
    if next.len() >= 2 {
        let last = next.last().unwrap().clone();
        let (status, _) = call(
            &app,
            &ben,
            "PUT",
            &format!("{}/order", cards(project)),
            json!({"status": "next", "ids": [last, Uuid::new_v4()]}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, board) = call(&app, &ana, "GET", &cards(project), json!(null)).await;
        let order: Vec<Value> = board["cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["status"] == "next")
            .map(|c| c["id"].clone())
            .collect();
        let mut expected = vec![last.clone()];
        expected.extend(next.iter().filter(|id| **id != last).cloned());
        assert_eq!(order, expected);
        let positions: Vec<i64> = board["cards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["status"] == "next")
            .map(|c| c["position"].as_i64().unwrap())
            .collect();
        assert_eq!(positions, (0..next.len() as i64).collect::<Vec<_>>());
    }
}
