//! Code steps: JavaScript run by QuickJS inside Chat, with no workspace pod. Each run
//! gets a fresh runtime with a memory cap, a stack cap and a deadline, and only the
//! language itself: no files, no network, no timers, no modules. The step's view of
//! the run (inputs, earlier steps, the current item) goes in as JSON, and what the
//! code returns comes back as JSON.
use rquickjs::{Context, Ctx, Error, Runtime};
use serde_json::Value;
use std::time::{Duration, Instant};

const MEMORY: usize = 64 * 1024 * 1024;
const STACK: usize = 1024 * 1024;
const TIME: Duration = Duration::from_secs(5);
/// What a code step may return, as JSON text.
const OUTPUT_BYTES: usize = 1024 * 1024;

/// Runs `code` as the body of a function that sees `inputs`, `steps`, `item` and
/// `index`, and returns what it returns.
pub async fn run(code: String, view: Value) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || evaluate(&format!("(function () {{\n{code}\n}})()"), &view))
        .await
        .map_err(|_| "The code stopped unexpectedly".to_string())?
}

/// Whether a JavaScript expression holds, for an If rule written as code.
pub async fn holds(expression: String, view: Value) -> Result<bool, String> {
    let value =
        tokio::task::spawn_blocking(move || evaluate(&format!("Boolean(\n{expression}\n)"), &view))
            .await
            .map_err(|_| "The rule stopped unexpectedly".to_string())??;
    Ok(value == Value::Bool(true))
}

fn evaluate(source: &str, view: &Value) -> Result<Value, String> {
    let runtime = Runtime::new().map_err(|_| "Couldn't start JavaScript".to_string())?;
    runtime.set_memory_limit(MEMORY);
    runtime.set_max_stack_size(STACK);
    let deadline = Instant::now() + TIME;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).map_err(|_| "Couldn't start JavaScript".to_string())?;
    context.with(|ctx| {
        let fail = |e: Error| describe(&ctx, e, deadline);
        let globals = ctx.globals();
        let given = ctx.json_parse(view.to_string()).map_err(fail)?;
        let given = given
            .as_object()
            .ok_or_else(|| "The step's inputs aren't an object".to_string())?;
        for name in ["inputs", "steps", "item", "index"] {
            let value: rquickjs::Value = given.get(name).map_err(fail)?;
            globals.set(name, value).map_err(fail)?;
        }
        let result: rquickjs::Value = ctx.eval(source.as_bytes().to_vec()).map_err(fail)?;
        if result.is_promise() {
            return Err("Code steps can't use await: return a value".into());
        }
        let text = ctx.json_stringify(result).map_err(fail)?;
        let Some(text) = text else {
            return Ok(Value::Null);
        };
        let text = text.to_string().map_err(fail)?;
        if text.len() > OUTPUT_BYTES {
            return Err("The code returned more than 1 MB".into());
        }
        serde_json::from_str(&text)
            .map_err(|_| "The code returned something that isn't JSON".into())
    })
}

/// A JavaScript error as one line the person can act on.
fn describe(ctx: &Ctx<'_>, error: Error, deadline: Instant) -> String {
    if Instant::now() > deadline {
        return format!("The code ran longer than {} seconds", TIME.as_secs());
    }
    if !error.is_exception() {
        return if matches!(error, Error::Allocation) {
            "The code used more than 64 MB".into()
        } else {
            error.to_string()
        };
    }
    let thrown = ctx.catch();
    if let Some(exception) = thrown.as_exception() {
        let message = exception.message().unwrap_or_default();
        if message.contains("out of memory") {
            return "The code used more than 64 MB".into();
        }
        let line = exception
            .stack()
            .and_then(|s| s.lines().next().map(str::trim).map(str::to_owned))
            .filter(|s| !s.is_empty());
        return match line {
            Some(at) => format!("{message} ({at})"),
            None => message,
        };
    }
    thrown
        .as_string()
        .and_then(|s| s.to_string().ok())
        .map(|s| format!("The code threw: {s}"))
        .unwrap_or_else(|| "The code threw an error".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view() -> Value {
        json!({"inputs": {"n": 3}, "steps": {"rows": [1, 2, 3]}, "item": null, "index": null})
    }

    #[tokio::test]
    async fn code_sees_the_run_and_returns_json() {
        let out = run(
            "return { total: steps.rows.reduce((a, b) => a + b, 0) * inputs.n };".into(),
            view(),
        )
        .await
        .unwrap();
        assert_eq!(out, json!({"total": 18}));
        assert_eq!(run("return;".into(), view()).await.unwrap(), Value::Null);
        assert!(holds("steps.rows.length > 2".into(), view()).await.unwrap());
        assert!(!holds("inputs.n === 4".into(), view()).await.unwrap());
    }

    #[tokio::test]
    async fn code_is_stopped_at_its_limits_and_reaches_nothing_outside() {
        let slow = run("while (true) {}".into(), view()).await.unwrap_err();
        assert!(slow.contains("longer than 5 seconds"), "{slow}");
        let big = run(
            "let a = []; while (true) a.push('x'.repeat(1e6));".into(),
            view(),
        )
        .await
        .unwrap_err();
        assert!(big.contains("64 MB"), "{big}");
        let thrown = run("throw new Error('bad sample');".into(), view())
            .await
            .unwrap_err();
        assert!(thrown.contains("bad sample"), "{thrown}");
        for missing in [
            "fetch('https://example.com')",
            "require('fs')",
            "setTimeout(() => 1, 1)",
            "std.open('/etc/passwd')",
        ] {
            let error = run(format!("return {missing};"), view()).await.unwrap_err();
            assert!(error.contains("not defined"), "{missing}: {error}");
        }
        let awaited = run("return Promise.resolve(1);".into(), view())
            .await
            .unwrap_err();
        assert!(awaited.contains("await"), "{awaited}");
    }
}
