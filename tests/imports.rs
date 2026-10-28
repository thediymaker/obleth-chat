//! History brought over from other apps, read from small synthetic exports.
use std::io::Write;

use axum::http::StatusCode;
use obleth_chat::store::{Conversation, ImportSummary, Store};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,username,display_name) VALUES($1,$2,'Import test')")
        .bind(id)
        .bind(format!("import-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn import(s: &Store, owner: Uuid, body: impl Into<Vec<u8>>) -> ImportSummary {
    s.import_upload(owner, body.into()).await.unwrap()
}

/// The owner's conversations, newest first.
async fn chats(s: &Store, owner: Uuid) -> Vec<Conversation> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE owner_id=$1 ORDER BY updated_at DESC,id",
    )
    .bind(owner)
    .fetch_all(&s.pool)
    .await
    .unwrap();
    let mut out = Vec::new();
    for id in ids {
        out.push(s.get(owner, id).await.unwrap());
    }
    out
}

fn contents(c: &Conversation) -> Vec<&str> {
    c.messages.iter().map(|m| m.content.as_str()).collect()
}

fn gpt_node(id: &str, parent: Option<&str>, message: Value) -> (String, Value) {
    (
        id.into(),
        json!({"id": id, "parent": parent, "children": [], "message": message}),
    )
}
fn gpt_message(role: &str, parts: Value, time: f64, extra: Value) -> Value {
    let mut m = json!({
        "id": "x", "author": {"role": role, "metadata": {}}, "create_time": time,
        "content": {"content_type": "text", "parts": parts},
        "status": "finished_successfully", "recipient": "all", "metadata": {}
    });
    if let Value::Object(extra) = extra {
        for (k, v) in extra {
            m[k] = v;
        }
    }
    m
}

/// A ChatGPT conversation with a hidden system node, a regenerated reply, an edited
/// prompt, a browsing step, reasoning, a continued reply and attachments.
fn chatgpt_branched() -> Value {
    let nodes = [
        gpt_node("root", None, Value::Null),
        gpt_node(
            "sys",
            Some("root"),
            gpt_message(
                "system",
                json!([""]),
                1_700_000_000.0,
                json!({"metadata": {"is_visually_hidden_from_conversation": true}}),
            ),
        ),
        gpt_node(
            "u1",
            Some("sys"),
            gpt_message(
                "user",
                json!(["First question"]),
                1_700_000_001.0,
                json!({}),
            ),
        ),
        gpt_node(
            "a1",
            Some("u1"),
            gpt_message(
                "assistant",
                json!(["First answer"]),
                1_700_000_010.0,
                json!({"metadata": {"model_slug": "gpt-4o"}}),
            ),
        ),
        gpt_node(
            "a1b",
            Some("u1"),
            gpt_message(
                "assistant",
                json!(["Regenerated answer"]),
                1_700_000_020.0,
                json!({"metadata": {"model_slug": "gpt-4o-mini"}}),
            ),
        ),
        gpt_node(
            "u2",
            Some("a1b"),
            gpt_message(
                "user",
                json!([{"content_type": "image_asset_pointer", "asset_pointer": "file-service://x"}, "Look at this"]),
                1_700_000_030.0,
                json!({"content": {"content_type": "multimodal_text", "parts": [{"content_type": "image_asset_pointer", "asset_pointer": "file-service://x"}, "Look at this"]},
                       "metadata": {"attachments": [{"name": "notes.pdf", "mime_type": "application/pdf"}, {"name": "photo.png", "mime_type": "image/png"}]}}),
            ),
        ),
        gpt_node(
            "u2b",
            Some("a1b"),
            gpt_message(
                "user",
                json!(["Edited follow-up"]),
                1_700_000_031.0,
                json!({}),
            ),
        ),
        gpt_node(
            "a2b",
            Some("u2b"),
            gpt_message(
                "assistant",
                json!(["Answer to the edit"]),
                1_700_000_032.0,
                json!({"metadata": {"model_slug": "gpt-4o"}}),
            ),
        ),
        gpt_node(
            "call",
            Some("u2"),
            gpt_message(
                "assistant",
                json!([]),
                1_700_000_040.0,
                json!({"recipient": "browser", "content": {"content_type": "code", "language": "unknown", "text": "search(\"x\")"}}),
            ),
        ),
        gpt_node(
            "result",
            Some("call"),
            gpt_message(
                "tool",
                json!([]),
                1_700_000_041.0,
                json!({"author": {"role": "tool", "name": "browser"}, "content": {"content_type": "tether_browsing_display", "result": "lots of pages"}}),
            ),
        ),
        gpt_node(
            "thought",
            Some("result"),
            gpt_message(
                "assistant",
                json!([]),
                1_700_000_042.0,
                json!({"content": {"content_type": "thoughts", "thoughts": [{"summary": "Plan", "content": "Thinking it over"}]}}),
            ),
        ),
        gpt_node(
            "a2",
            Some("thought"),
            gpt_message(
                "assistant",
                json!(["Part one"]),
                1_700_000_050.0,
                json!({"metadata": {"model_slug": "gpt-4o"}}),
            ),
        ),
        gpt_node(
            "a2more",
            Some("a2"),
            gpt_message("assistant", json!(["Part two"]), 1_700_000_051.0, json!({})),
        ),
    ];
    json!({
        "title": "Branched chat",
        "create_time": 1_700_000_000.5,
        "update_time": 1_700_000_100.0,
        "current_node": "a2more",
        "conversation_id": "conv-1",
        "id": "conv-1",
        "is_archived": true,
        "mapping": nodes.into_iter().collect::<serde_json::Map<_, _>>(),
    })
}

/// A ChatGPT image request: the reply is the generated image.
fn chatgpt_image() -> Value {
    let nodes = [
        gpt_node(
            "u",
            None,
            gpt_message("user", json!(["Draw a cat"]), 1_710_000_000.0, json!({})),
        ),
        gpt_node(
            "call",
            Some("u"),
            gpt_message(
                "assistant",
                json!(["{\"prompt\":\"cat\"}"]),
                1_710_000_001.0,
                json!({"recipient": "dalle.text2im"}),
            ),
        ),
        gpt_node(
            "img",
            Some("call"),
            gpt_message(
                "tool",
                json!([]),
                1_710_000_002.0,
                json!({"content": {"content_type": "multimodal_text", "parts": [{"content_type": "image_asset_pointer", "asset_pointer": "sediment://y"}]}}),
            ),
        ),
        gpt_node(
            "after",
            Some("img"),
            gpt_message("assistant", json!([""]), 1_710_000_003.0, json!({})),
        ),
    ];
    json!({
        "title": "", "create_time": 1_710_000_000.0, "update_time": 1_710_000_003.0,
        "current_node": "after", "id": "conv-2",
        "mapping": nodes.into_iter().collect::<serde_json::Map<_, _>>(),
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn chatgpt_branches_become_versions_on_the_current_path(pool: PgPool) {
    let owner = user(&pool).await;
    let s = Store::new(pool.clone());
    let file = serde_json::to_vec(&json!([chatgpt_branched(), chatgpt_image()])).unwrap();
    let summary = import(&s, owner, file.clone()).await;
    assert_eq!(summary.source, "chatgpt");
    assert_eq!(summary.imported, 2);
    assert_eq!(summary.alternatives_left_out, 0);
    let all = chats(&s, owner).await;
    let image = &all[0];
    assert_eq!(image.title, "Draw a cat");
    assert_eq!(contents(image), ["Draw a cat", "[Image not imported]"]);
    let c = &all[1];
    assert_eq!(c.title, "Branched chat");
    assert!(c.archived);
    assert_eq!(c.model, "auto");
    assert_eq!(c.updated_at.timestamp(), 1_700_000_100);
    // The hidden system prompt, the browsing step and the reasoning step fold away;
    // the continued reply is one message with its reasoning.
    assert_eq!(c.messages.len(), 4);
    assert_eq!(c.messages[0].content, "First question");
    assert_eq!(c.messages[0].role, "user");
    assert_eq!(c.messages[0].created_at.timestamp(), 1_700_000_001);
    assert_eq!(c.messages[1].content, "Regenerated answer");
    assert_eq!(
        c.messages[1].settings,
        Some(json!({"model": "gpt-4o-mini"}))
    );
    assert_eq!(
        (c.messages[1].versions.len(), c.messages[1].version_index),
        (2, 2)
    );
    assert_eq!(
        c.messages[2].content,
        "[Image not imported]\n\nLook at this\n\n[File not imported: notes.pdf]"
    );
    assert_eq!(
        (c.messages[2].versions.len(), c.messages[2].version_index),
        (2, 1)
    );
    assert_eq!(c.messages[3].content, "Part one\n\nPart two");
    assert_eq!(c.messages[3].reasoning_content, "Thinking it over");
    assert_eq!(c.messages[3].settings, Some(json!({"model": "gpt-4o"})));
    assert_eq!(c.messages[3].status, "completed");
    // Switching to the edited prompt shows its own answer; switching back restores
    // the path the export ended on.
    let edit = c.messages[2].versions[1].id;
    let switched = s.select_version(owner, c.id, edit).await.unwrap();
    assert_eq!(
        contents(&switched),
        [
            "First question",
            "Regenerated answer",
            "Edited follow-up",
            "Answer to the edit"
        ]
    );
    let first = c.messages[1].versions[0].id;
    let switched = s.select_version(owner, c.id, first).await.unwrap();
    assert_eq!(contents(&switched), ["First question", "First answer"]);
    let back = s
        .select_version(owner, c.id, c.messages[2].id)
        .await
        .unwrap();
    assert_eq!(back.messages.len(), 4);
    assert_eq!(back.messages[3].content, "Part one\n\nPart two");

    // The chat continues with the person's own model.
    let turn = s
        .start_turn(owner, image.id, "And a dog", "fixture", "", 0.7, 256)
        .await
        .unwrap();
    assert_eq!(turn.messages.len(), 3);
    assert_eq!(turn.messages[1].content, "[Image not imported]");
    // Regenerating an imported reply carries no foreign settings into the request.
    let context = s
        .variant_context(owner, c.id, c.messages[3].id, "regenerate")
        .await
        .unwrap();
    assert_eq!(context.settings, json!({}));

    // The same file again adds nothing.
    let again = import(&s, owner, file).await;
    assert_eq!((again.imported, again.already_imported), (0, 2));
    assert_eq!(chats(&s, owner).await.len(), 2);
}

/// A minimal zip writer: each entry stored or deflated, with its checksum.
fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for &(name, data, deflate) in entries {
        let body = if deflate {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        } else {
            data.to_vec()
        };
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let method: u16 = if deflate { 8 } else { 0 };
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(method.to_le_bytes());
            h.extend([0u8; 4]);
            h.extend(crc.to_le_bytes());
            h.extend((body.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 6]);
                h.extend(0u32.to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend(&body);
        directory.extend(header(0x0201_4b50, true));
    }
    let at = out.len() as u32;
    out.extend(&directory);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((directory.len() as u32).to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[sqlx::test(migrations = "./migrations")]
async fn chatgpt_zip_and_damaged_or_unknown_files(pool: PgPool) {
    let owner = user(&pool).await;
    let s = Store::new(pool);
    let json = serde_json::to_vec(&json!([chatgpt_image()])).unwrap();
    for deflate in [true, false] {
        let file = zip(&[
            ("chat.html", b"<html></html>", true),
            ("export/conversations.json", &json, deflate),
            ("__MACOSX/conversations.json", b"junk", false),
        ]);
        let summary = import(&s, owner, file).await;
        assert_eq!(summary.source, "chatgpt");
        assert_eq!(summary.imported + summary.already_imported, 1);
    }
    assert_eq!(chats(&s, owner).await.len(), 1);
    let mut damaged = zip(&[("conversations.json", &json, false)]);
    // Still valid JSON, so only the checksum can tell.
    let at = damaged.windows(4).position(|w| w == b"Draw").unwrap();
    damaged[at] = b'd';
    let without = zip(&[("user.json", b"{}", false)]);
    for (file, says) in [
        (damaged, "could not be read"),
        (without, "no conversations.json"),
        (b"PK\x03\x04 not really".to_vec(), "could not be read"),
        (b"hello".to_vec(), "isn't a chat export"),
        (br#"[{"foo": 1}]"#.to_vec(), "isn't a chat export"),
        (b"[1, 2]".to_vec(), "isn't a chat export"),
        (b"[]".to_vec(), "no chats"),
        (br#"[{"mapping": {}}"#.to_vec(), "isn't a chat export"),
        (
            br#"{"format": "something-else"}"#.to_vec(),
            "Obleth Chat v1",
        ),
    ] {
        let error = s.import_upload(owner, file).await.unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(error.1.contains(says), "{}", error.1);
    }
    // A conversation with nothing to show is counted, not imported.
    let summary = import(&s, owner, br#"[{"mapping": {}, "id": "blank"}]"#.to_vec()).await;
    assert_eq!((summary.imported, summary.empty), (0, 1));
}

#[sqlx::test(migrations = "./migrations")]
async fn open_webui_keeps_versions_reasoning_pins_and_placeholders(pool: PgPool) {
    let owner = user(&pool).await;
    let s = Store::new(pool);
    let message = |id: &str, parent: Option<&str>, role: &str, content: &str, extra: Value| {
        let mut m = json!({"id": id, "parentId": parent, "childrenIds": [], "role": role,
            "content": content, "timestamp": 1_720_000_000 + id.len() as i64});
        if let Value::Object(extra) = extra {
            for (k, v) in extra {
                m[k] = v;
            }
        }
        (id.to_owned(), m)
    };
    let messages: serde_json::Map<String, Value> = [
        message("u1", None, "user", "Compare these", json!({"files": [{"type": "image", "url": "data:image/png;base64,AAAA"}, {"type": "file", "name": "plan.docx"}]})),
        message("a1", Some("u1"), "assistant",
            "<details type=\"reasoning\" done=\"true\" duration=\"2\">\n<summary>Thought for 2 seconds</summary>\n> Let me think\n> carefully\n</details>\nThe answer",
            json!({"model": "llama3:8b", "modelIdx": 0})),
        message("a1-other", Some("u1"), "assistant", "Another model's answer", json!({"model": "qwen2:7b", "modelIdx": 1})),
        message("u2", Some("a1"), "user", "Thanks", json!({})),
        message("a2", Some("u2"), "assistant",
            "<details type=\"tool_calls\" done=\"true\" id=\"t\">\n<summary>Tool Executed</summary>\n</details>\nYou're welcome",
            json!({"model": "llama3:8b"})),
        message("a3", Some("u2"), "assistant", "", json!({"error": {"content": "Model not found"}, "model": "gone"})),
    ]
    .into_iter()
    .collect();
    let file = json!([
        {"id": "chat-1", "user_id": "u", "title": "Model comparison", "created_at": 1_720_000_000, "updated_at": 1_720_000_500,
         "archived": false, "pinned": true, "meta": {"tags": ["x"]}, "folder_id": null,
         "chat": {"title": "Model comparison", "models": ["llama3:8b", "qwen2:7b"],
                  "history": {"currentId": "a2", "messages": messages}, "messages": []}},
        {"id": "chat-2", "title": "Old style", "created_at": 1_600_000_000, "updated_at": 1_600_000_100,
         "chat": {"messages": [{"role": "user", "content": "Hi"}, {"role": "system", "content": "hidden"}, {"role": "assistant", "content": "Hello"}]}}
    ]);
    let file = serde_json::to_vec(&file).unwrap();
    let summary = import(&s, owner, file.clone()).await;
    assert_eq!((summary.source, summary.imported), ("open-webui", 2));
    let all = chats(&s, owner).await;
    let c = &all[0];
    assert!(c.pinned && !c.archived);
    assert_eq!(c.title, "Model comparison");
    assert_eq!(
        contents(c),
        [
            "Compare these\n\n[Image not imported]\n\n[File not imported: plan.docx]",
            "The answer",
            "Thanks",
            "You're welcome"
        ]
    );
    assert_eq!(c.messages[1].reasoning_content, "Let me think\ncarefully");
    assert_eq!(c.messages[1].settings, Some(json!({"model": "llama3:8b"})));
    // The second model's answer is a version of the reply.
    assert_eq!(c.messages[1].versions.len(), 2);
    // A failed reply keeps its error as a version.
    assert_eq!(c.messages[3].versions.len(), 2);
    let failed = c.messages[3].versions[1].id;
    let switched = s.select_version(owner, c.id, failed).await.unwrap();
    assert_eq!(switched.messages[3].status, "error");
    assert_eq!(
        switched.messages[3].error_message.as_deref(),
        Some("Model not found")
    );
    let old = &all[1];
    assert_eq!(contents(old), ["Hi", "Hello"]);
    assert_eq!(old.updated_at.timestamp(), 1_600_000_100);
    let again = import(&s, owner, file).await;
    assert_eq!((again.imported, again.already_imported), (0, 2));
}

#[sqlx::test(migrations = "./migrations")]
async fn claude_lists_and_linked_branches(pool: PgPool) {
    let owner = user(&pool).await;
    let s = Store::new(pool);
    let root = "00000000-0000-4000-8000-000000000000";
    let file = json!([
        {"uuid": "c1", "name": "Spreadsheet help", "created_at": "2025-01-02T03:04:05.000000Z",
         "updated_at": "2025-01-02T04:00:00Z", "account": {"uuid": "acct"},
         "chat_messages": [
            {"uuid": "m1", "sender": "human", "text": "Sum this column", "content": [{"type": "text", "text": "Sum this column"}],
             "created_at": "2025-01-02T03:04:06Z", "attachments": [{"file_name": "data.csv", "extracted_content": "1\n2"}], "files": [{"file_name": "chart.png"}]},
            {"uuid": "m2", "sender": "assistant", "text": "ignored when content has text",
             "content": [{"type": "thinking", "thinking": "Add them"}, {"type": "tool_use", "name": "repl"}, {"type": "tool_result", "content": []}, {"type": "text", "text": "The sum is 3."}],
             "created_at": "2025-01-02T03:04:07Z"}
         ]},
        {"uuid": "c2", "name": "", "created_at": "2025-02-01T00:00:00Z", "updated_at": "2025-02-01T00:10:00Z",
         "chat_messages": [
            {"uuid": "q", "parent_message_uuid": root, "sender": "human", "text": "Name a color", "created_at": "2025-02-01T00:00:01Z"},
            {"uuid": "r1", "parent_message_uuid": "q", "sender": "assistant", "text": "Blue", "created_at": "2025-02-01T00:00:02Z"},
            {"uuid": "r2", "parent_message_uuid": "q", "sender": "assistant", "text": "Green", "created_at": "2025-02-01T00:00:03Z"}
         ]}
    ]);
    let summary = import(&s, owner, serde_json::to_vec(&file).unwrap()).await;
    assert_eq!((summary.source, summary.imported), ("claude", 2));
    let all = chats(&s, owner).await;
    let branched = &all[0];
    assert_eq!(branched.title, "Name a color");
    // Without a recorded current message, the latest reply is shown.
    assert_eq!(contents(branched), ["Name a color", "Green"]);
    assert_eq!(branched.messages[1].versions.len(), 2);
    assert_eq!(branched.messages[1].settings, None);
    let listed = &all[1];
    assert_eq!(
        contents(listed),
        [
            "Sum this column\n\n[File not imported: data.csv]\n\n[Image not imported]",
            "The sum is 3."
        ]
    );
    assert_eq!(listed.messages[1].reasoning_content, "Add them");
    assert_eq!(
        listed.messages[0].created_at.to_rfc3339(),
        "2025-01-02T03:04:06+00:00"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn chat_exports_still_import_through_the_upload(pool: PgPool) {
    let alice = user(&pool).await;
    let bob = user(&pool).await;
    let s = Store::new(pool);
    let c = s.create(alice, "Portable", "fixture").await.unwrap();
    let turn = s
        .start_turn(alice, c.id, "question", "fixture", "", 0.7, 256)
        .await
        .unwrap();
    s.finish_turn(
        alice,
        c.id,
        turn.generation_id,
        "answer",
        "completed",
        None,
        None,
    )
    .await
    .unwrap();
    let archive = serde_json::to_vec(&s.export(alice, &[c.id]).await.unwrap()).unwrap();
    let summary = import(&s, bob, archive.clone()).await;
    assert_eq!((summary.source, summary.imported), ("obleth-chat", 1));
    let copy = summary.conversations.unwrap()[0].id;
    assert_eq!(
        contents(&s.get(bob, copy).await.unwrap()),
        ["question", "answer"]
    );
    // Chat's own exports are copies, so importing twice makes two.
    assert_eq!(import(&s, bob, archive).await.imported, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn limits_on_files_chats_and_long_transcripts(pool: PgPool) {
    let owner = user(&pool).await;
    let s = Store::new(pool.clone());
    let error = s
        .import_upload(owner, vec![0u8; 200 * 1024 * 1024 + 1])
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::PAYLOAD_TOO_LARGE);
    let many = format!("[{}]", vec![r#"{"mapping":{}}"#; 5_001].join(","));
    let error = s.import_upload(owner, many).await.unwrap_err();
    assert!(error.1.contains("5,000"), "{}", error.1);

    // A transcript longer than Chat keeps brings its latest 200 messages, and a
    // message over the size limit is shortened.
    let mut nodes = serde_json::Map::new();
    for i in 0..230 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let text = if i == 229 {
            "y".repeat(1_100_000)
        } else {
            format!("message {i}")
        };
        let parent = (i > 0).then(|| format!("n{}", i - 1));
        let (k, v) = gpt_node(
            &format!("n{i}"),
            parent.as_deref(),
            gpt_message(role, json!([text]), 1_730_000_000.0 + i as f64, json!({})),
        );
        nodes.insert(k, v);
    }
    let long = json!([{"title": "Long", "id": "long", "current_node": "n229", "mapping": nodes,
                       "create_time": 1_730_000_000.0, "update_time": 1_730_001_000.0}]);
    let summary = import(&s, owner, serde_json::to_vec(&long).unwrap()).await;
    assert_eq!((summary.imported, summary.messages_left_out), (1, 30));
    let c = &chats(&s, owner).await[0];
    assert_eq!(c.messages.len(), 200);
    assert_eq!(c.messages[0].content, "message 30");
    let last = &c.messages[199].content;
    assert!(last.ends_with("[Shortened on import]") && last.len() <= 1024 * 1024);

    // At the conversation limit, the newest chats come over and the rest are counted.
    sqlx::query("INSERT INTO conversations(id,owner_id,title,model) SELECT gen_random_uuid(),$1,'Filler','fixture' FROM generate_series(1,997)")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let chat = |id: &str, time: f64| {
        json!({"id": id, "title": id, "update_time": time, "current_node": "u",
               "mapping": {"u": {"id": "u", "parent": null, "message": gpt_message("user", json!(["hi"]), time, json!({}))}}})
    };
    let file = json!([
        chat("older", 1_700_000_000.0),
        chat("newest", 1_720_000_000.0),
        chat("newer", 1_710_000_000.0)
    ]);
    let summary = import(&s, owner, serde_json::to_vec(&file).unwrap()).await;
    assert_eq!((summary.imported, summary.over_limit), (2, 1));
    let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM conversations WHERE owner_id=$1 AND import_key IS NOT NULL AND title<>'Long' ORDER BY updated_at DESC")
        .bind(owner)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(titles, ["newest", "newer"]);
}
