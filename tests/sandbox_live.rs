//! Live check against a real cluster. Run with the workspace environment variables
//! set and OBLETH_CHAT_SANDBOX_TEST_SLUG naming a running workspace:
//! cargo test --test sandbox_live -- --ignored --nocapture
use obleth_chat::sandbox::{tools, SandboxConfig, Sandboxes};
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[ignore]
async fn tools_run_inside_a_live_workspace() {
    let pool = PgPoolOptions::new()
        .connect(&std::env::var("OBLETH_CHAT_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let sb = Sandboxes::new(SandboxConfig::from_env().unwrap().unwrap(), pool).unwrap();
    let slug = std::env::var("OBLETH_CHAT_SANDBOX_TEST_SLUG").unwrap();
    let sandbox = sb.by_slug(&slug).await.unwrap().unwrap();
    let sandbox = sb.ensure_running(&sandbox).await.unwrap();
    let run = |name: &'static str, args: serde_json::Value| {
        let sb = sb.clone();
        let sandbox = sandbox.clone();
        async move {
            let outcome = tools::run(&sb, &sandbox, name, &args.to_string()).await;
            println!(
                "{name}: ok={} summary={} detail={}\n{}\n",
                outcome.ok, outcome.summary, outcome.detail, outcome.output
            );
            outcome
        }
    };
    assert!(
        run(
            "write_file",
            serde_json::json!({"path":"site/index.html","content":"<h1>Hello α</h1>\n"})
        )
        .await
        .ok
    );
    assert!(
        run(
            "edit_file",
            serde_json::json!({"path":"site/index.html","old_text":"Hello","new_text":"Hi"})
        )
        .await
        .ok
    );
    let read = run(
        "read_file",
        serde_json::json!({"path":"/workspace/site/index.html"}),
    )
    .await;
    assert!(read.output.contains("<h1>Hi α</h1>"));
    assert!(run("list_files", serde_json::json!({}))
        .await
        .output
        .contains("site/index.html"));
    let failed = run(
        "run_command",
        serde_json::json!({"command":"echo out; echo err >&2; exit 3"}),
    )
    .await;
    assert!(!failed.ok && failed.output.contains("exit code 3") && failed.output.contains("err"));
    assert!(
        run(
            "run_command",
            serde_json::json!({"command":"node --version && whoami && pwd"})
        )
        .await
        .ok
    );
    let server = run("start_server", serde_json::json!({"command":"cd site && python3 -m http.server 3000 --bind 0.0.0.0","port":3000})).await;
    // The host running this test may not route to pod addresses; inside the cluster this succeeds.
    println!("start_server reachable from here: {}", server.ok);
    assert!(run("server_logs", serde_json::json!({"lines":5})).await.ok);
}
