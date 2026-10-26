//! Agent tools that run inside a workspace pod. Every tool returns text for the
//! model plus a short summary and detail for the activity list in the UI.
use super::{Sandbox, Sandboxes};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Output returned to the model per tool call.
const MODEL_OUTPUT: usize = 12 * 1024;
// Per-project server files; every script runs with OBLETH_SLUG set (see `sh`).
const SERVER_LOG: &str = "/tmp/obleth-$OBLETH_SLUG.log";
const SERVER_PID: &str = "/tmp/obleth-$OBLETH_SLUG.pid";
const SERVER_PORT: &str = "/tmp/obleth-$OBLETH_SLUG.port";
const SERVER_KIND: &str = "/tmp/obleth-$OBLETH_SLUG.kind";

/// Serves the project folder without caching, so a reload always shows the latest files.
const STATIC_SERVER: &str = "python3 -c 'import http.server as h, sys
class H(h.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header(\"Cache-Control\", \"no-store\")
        super().end_headers()
h.ThreadingHTTPServer((\"0.0.0.0\", int(sys.argv[1])), H).serve_forever()'";

pub struct Outcome {
    pub ok: bool,
    pub summary: String,
    pub output: String,
    pub detail: String,
    pub preview: Option<u16>,
    /// Facts the chat shows about the step: the file and lines a write changed,
    /// whether the preview compiled after it, or the files a plan names.
    pub extra: Value,
}

pub fn definitions() -> Value {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| json!({"type":"function","function":{"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}});
    json!([
        tool("list_files", "List files and directories under a path in the workspace (node_modules, .next and .git are skipped).", json!({"path":{"type":"string","description":"Directory relative to the project folder. Defaults to the project root."}}), &[]),
        tool("read_file", "Read a text file from the workspace. Each line comes back prefixed with its line number and a colon, like `12: text`; the prefix is not part of the file. Long files come back in windows: the result ends by saying which offset to read next. Read the whole window you need in one call rather than many small slices.", json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":1,"description":"First line to read, from 1. Defaults to 1."},"limit":{"type":"integer","minimum":1,"maximum":2000,"description":"Number of lines. Defaults to as many as fit."}}), &["path"]),
        tool("write_file", "Create a file, or replace one completely, with the full new content. Parent directories are created. For a change to part of an existing file use edit_file. Write a long file in parts: the first with write_file, each next one with append set.", json!({"path":{"type":"string"},"content":{"type":"string"},"append":{"type":"boolean","description":"Add the content to the end of the file instead of replacing it."}}), &["path","content"]),
        tool("edit_file", "Replace old_text with new_text in a file you have read. Copy old_text from the file exactly, without the line-number prefixes read_file adds, and include enough surrounding lines that it occurs only once (or set replace_all to change every occurrence). Differences in indentation or trailing spaces are tolerated.", json!({"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"replace_all":{"type":"boolean","description":"Replace every occurrence. Defaults to false."}}), &["path","old_text","new_text"]),
        tool("run_command", "Run a shell command in the project folder and return its output. Use for installs, builds, tests and inspection. Do not use it to start or stop servers.", json!({"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":300}}), &["command"]),
        tool("start_server", "Start or restart a long-running server and show it in the preview pane. Next.js and Vite dev servers, and a plain web server for an index.html site, start automatically, so only use this for other servers, or to restart after changing package.json or the framework config. It must listen on 0.0.0.0 at the given port.", json!({"command":{"type":"string","description":"For example: python3 -m http.server 3000 --bind 0.0.0.0"},"port":{"type":"integer","minimum":1024,"maximum":65535}}), &["command","port"]),
        tool("run_in_terminal", "Open the person's Terminal tab and run a command there, for programs they use with the keyboard: terminal UIs (ratatui, crossterm, curses, Textual, bubbletea), games, REPLs and prompts. Build and test with run_command first. You do not see the output.", json!({"command":{"type":"string","description":"One line, such as `cargo run --release` or `python app.py`. A program already running in the terminal is stopped first."}}), &["command"]),
        tool("server_logs", "Show the latest output of the server started with start_server.", json!({"lines":{"type":"integer","minimum":1,"maximum":400}}), &[]),
        tool("plan", "Say which files you are about to create or change, before writing several. The user sees the list while you work. Call it again if the plan changes.", json!({"files":{"type":"array","items":{"type":"string"},"description":"Paths relative to the project folder"}}), &["files"]),
    ])
}

/// How the workspace agent works. People can replace this with their own in Settings;
/// the facts about the workspace in `system_prompt` always apply.
pub const DEFAULT_INSTRUCTIONS: &str = "# How to work
You are a careful software engineer working for the person in this chat. Act through your tools: when a request involves creating, changing or running code or files, make the change with the tools instead of describing it. Code that appears only in your reply is not saved anywhere. When a message could be a question or a task, treat it as a task unless it clearly asks only for an explanation.

## Before you change anything
- Look first: list the project and read the files you are about to change. Never edit a file you have not read in this conversation. If an edit does not match, read the file again before retrying.
- Follow what is already there: match the project's structure, naming, style and libraries. Check package.json or requirements.txt before using a library; never assume one is installed.
- Before adding a component or module, read a similar existing one and write yours the same way.
- For work that touches several files, call plan with the files first, then work through them one at a time.

## While you work
- Make the smallest change that does what was asked, and keep it simple. Do not add features, files, abstractions or documentation nobody asked for. Prefer editing an existing file to creating a new one.
- Use edit_file for changes to part of a file and write_file for new files or complete rewrites.
- Fix causes, not symptoms. When something fails, read the error, change the code and try again. Do not give up after one failure, and do not repeat a step that failed the same way twice: try a different approach or explain what is blocking you.
- When you say you will do something, do it in the same reply with a tool call instead of ending your reply.
- Ask one short question only when you cannot continue without the answer, such as a missing credential or a choice that changes the result. Otherwise make a sensible choice and mention it.
- Do not add comments that only narrate the code.

## Before you reply
- Verify your work: check the tool results for compile and syntax errors, run the project's tests or a quick command that exercises the change when there is one, and fix what fails.
- Stop only when the request is done or you are truly blocked.
- Keep the final reply short and plain: what you built or changed, the files that matter, anything that did not work, and what the person can try next. Do not paste the code you wrote or repeat tool output.";

/// The workspace agent's system prompt: facts about this workspace, then how to work
/// (the person's own instructions or the default), then what the project holds now.
pub fn system_prompt(sandbox: &Sandbox, instructions: Option<&str>, context: &str) -> String {
    let facts = format!(
        "# Your workspace
- A Linux workspace (Debian, Node.js 22 with npm, Python 3, Rust, Go, git) that belongs to this conversation. The project folder is {dir}; commands run there and files persist between messages. Other folders in /workspace belong to the person's other conversations: leave them alone.
- Your tools are function calls, never shell commands: do not type start_server, write_file and the like into run_command.
- Use the stack the person asks for. When they do not name one, build web apps and sites with Vite, React, TypeScript and Tailwind CSS; use Next.js only when asked or when the app needs server-side code such as API routes, auth or a database, and plain HTML for a single static page.
- Starters with dependencies already installed: Vite at /opt/templates/vite (edit src/App.tsx) and Next.js (App Router) at /opt/templates/nextjs (edit app/page.tsx). To start a project in an empty folder, copy one with `cp -r /opt/templates/vite/. .` (or nextjs) instead of create-vite, create-next-app or npm install.
- Chat runs the dev server for you: once the project holds a Next.js or Vite app with its dependencies installed, Chat starts `npm run dev` on port {port} and the person sees it live in a preview pane next to the chat. It reloads on file changes. A plain HTML site (an HTML file in the project folder and no package.json dev script) needs no server either: Chat serves the folder on port {port} and reloads the preview after each change.
- Never start, stop or kill the dev server yourself (no `npm run dev`, kill or pkill). Call start_server only for other kinds of servers (on port {port}) or to restart after changing package.json or the framework config.
- After every file change the tool result says whether the preview still compiles, or for a plain HTML site whether the script parses. If it reports an error, fix it before replying.
- The preview pane shows any web server, in any language: Flask, FastAPI (uvicorn), Streamlit or Gradio in Python, axum or actix in Rust, net/http in Go, and so on. It must listen on 0.0.0.0 port {port}; start it with start_server. For a Jupyter notebook, start `jupyter lab --ip 0.0.0.0 --port {port} --no-browser --IdentityProvider.token= --ServerApp.tornado_settings='{{\"headers\":{{\"Content-Security-Policy\":\"frame-ancestors *\"}}}}'` with start_server.
- Terminal apps (TUIs with ratatui, crossterm, curses, Textual or bubbletea, terminal games, REPLs, anything that reads the keyboard) run in the person's Terminal tab: build and test them with run_command, then call run_in_terminal with the command so they can use the app there. Never run them with run_command, which has no keyboard and would wait until it times out, and never rebuild a terminal app as a web page unless asked.
- Desktop GUI windows (Tkinter, Qt, pygame) cannot be shown yet: say so, and offer a terminal or web version.
- For scripts, data work and class assignments no web app is needed: Python 3 with numpy, pandas, matplotlib, scipy, scikit-learn, pytest, Jupyter, Flask, FastAPI and uvicorn is installed, as are uv, Rust (cargo, with rustfmt and clippy), Go, gcc, g++ and make. The system Python is read-only, so a plain `pip install` fails with Permission denied and `--user` is refused: for any other package, first create a project environment with `python3 -m venv --system-site-packages .venv`, then use `.venv/bin/pip install …` and `.venv/bin/python`. It lives in the project, so it survives the workspace sleeping. Very large packages such as PyTorch can run out of the workspace's memory while installing; say so and choose smaller ones where you can. Save plots and results as files (PNG, SVG or CSV) in the project and tell the person their names: they open them in the Files tab of the workspace pane.
- Public git repositories can be cloned over HTTPS. To read, explain or review one the person points at, clone it into repos/ (`git clone --depth 50 <url> repos/<name>`), which stays out of this project's own history; for a big codebase, hand the reading to workers with delegate if you have it. To build on a repository as a project's code with its history, its owner can start an empty project from it on the project's Code tab (Start from a repository); otherwise copy its files into the project folder without its .git.
- Package downloads may be slow or unavailable; prefer what is installed. `uv pip install --python .venv/bin/python …` installs into a project environment much faster than pip. Cargo and Go download crates and modules on first build, which can take a minute; build output stays in the project.
- Only tell the person an app is running or visible after the dev server or start_server reported success; if something failed, say what failed.
- An AGENTS.md file in the project folder holds the person's notes for this project; follow it, and keep it current when you change something it describes.",
        dir = sandbox.dir(),
        port = sandbox.port
    );
    let how = instructions
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(DEFAULT_INSTRUCTIONS);
    let mut prompt = format!("{facts}\n\n{how}");
    if !context.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(context);
    }
    prompt
}

/// Today's date, the project's files and its AGENTS.md, for the start of a turn.
/// Empty when the workspace is asleep; the model can list files once it wakes.
pub async fn project_context(sb: &Sandboxes, sandbox: &Sandbox) -> String {
    let mut context = format!(
        "# Right now\nToday is {}.",
        chrono::Utc::now().format("%A %-d %B %Y")
    );
    if sandbox.status != "running" || sandbox.pod_ip.is_none() {
        return context;
    }
    let script = "find . -mindepth 1 -maxdepth 4 \\( -name node_modules -o -name .next -o -name .git -o -name dist -o -name build -o -name .venv -o -name __pycache__ \\) -prune -o \\( -type d -printf '%P/\\n' \\) -o \\( -type f -printf '%P\\n' \\) 2>/dev/null | sort | head -n 201; \
        echo '@@obleth-agents@@'; [ -f AGENTS.md ] && head -c 8192 AGENTS.md";
    let Ok((out, _, _)) = sh(sb, sandbox, script, &[], None, Duration::from_secs(10)).await else {
        return context;
    };
    let (files, agents) = out.split_once("@@obleth-agents@@").unwrap_or((&out, ""));
    let files: Vec<&str> = files.lines().filter(|l| !l.trim().is_empty()).collect();
    if files.is_empty() {
        context.push_str(" The project folder is empty.");
    } else {
        context.push_str(
            "\n\nFiles in the project folder (dependencies and build output left out):\n",
        );
        for file in files.iter().take(200) {
            context.push_str(file);
            context.push('\n');
        }
        if files.len() > 200 {
            context.push_str("… and more; use list_files to see the rest.\n");
        }
    }
    let agents = agents.trim();
    if !agents.is_empty() {
        context.push_str(&format!("\nThe project's AGENTS.md:\n{agents}"));
    }
    context.trim_end().to_owned()
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A path relative to the project folder; absolute paths under the project folder
/// or /workspace are accepted. `..` is refused, so file tools stay in the project.
fn clean_path(path: &str, slug: &str) -> Result<String, String> {
    let path = path.trim();
    if path.len() > 512 || path.chars().any(char::is_control) {
        return Err("Invalid path".into());
    }
    if path.split('/').any(|part| part == "..") {
        return Err("Paths must stay inside the project; `..` is not allowed".into());
    }
    let project = format!("/workspace/{slug}");
    let relative = path
        .strip_prefix(project.as_str())
        .filter(|rest| !slug.is_empty() && (rest.is_empty() || rest.starts_with('/')))
        .or_else(|| path.strip_prefix("/workspace"))
        .unwrap_or(path)
        .trim_start_matches('/');
    Ok(if relative.is_empty() {
        ".".into()
    } else {
        relative.to_owned()
    })
}

fn shorten(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[{} earlier bytes omitted]\n{}", start, &text[start..])
}

fn one_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>())
    } else {
        line.to_owned()
    }
}

pub(crate) async fn sh(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    script: &str,
    args: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<(String, String, Option<i32>), String> {
    let script = in_project(sb, sandbox, script);
    let mut argv = vec!["sh", "-c", script.as_str(), "sh"];
    argv.extend_from_slice(args);
    let output = tokio::time::timeout(timeout, sb.exec(sandbox, &argv, stdin))
        .await
        .map_err(|_| "The command did not finish in time".to_string())?
        .map_err(|error| format!("Could not run the command in the workspace: {error:#}"))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.exit_code,
    ))
}

/// Every script runs in the conversation's project folder, which may not exist yet.
pub(crate) fn in_project(sb: &Sandboxes, sandbox: &Sandbox, script: &str) -> String {
    format!(
        "export OBLETH_SLUG={slug} OBLETH_PREVIEW_ORIGIN={origin}; mkdir -p {dir} && cd {dir} || exit 1\n{script}",
        slug = sandbox.slug,
        origin = sb.preview_origin(sandbox).unwrap_or_default(),
        dir = sandbox.dir(),
    )
}

/// Largest project zip served, without dependencies and build output.
const ARCHIVE_LIMIT: usize = 64 * 1024 * 1024;
const ARCHIVE_SCRIPT: &str = r#"
import os, sys, zipfile
skip = {"node_modules", ".next", "dist", "build", ".venv", "venv", "__pycache__", ".cache", ".turbo"}
with zipfile.ZipFile(sys.stdout.buffer, "w", zipfile.ZIP_DEFLATED) as z:
    for root, dirs, files in os.walk("."):
        dirs[:] = sorted(d for d in dirs if d not in skip)
        for name in sorted(files):
            path = os.path.join(root, name)
            if os.path.isfile(path) and not os.path.islink(path):
                z.write(path, os.path.relpath(path, "."))
"#;

/// The project folder as a zip archive.
pub async fn archive(sb: &Sandboxes, sandbox: &Sandbox) -> Result<Vec<u8>, String> {
    let script = in_project(sb, sandbox, "exec python3 -c \"$1\"");
    let argv = ["sh", "-c", script.as_str(), "sh", ARCHIVE_SCRIPT];
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        sb.exec_limited(sandbox, &argv, ARCHIVE_LIMIT),
    )
    .await
    .map_err(|_| "Packing the project took too long".to_string())?
    .map_err(|error| format!("Could not pack the project: {error:#}"))?;
    if output.truncated {
        return Err(
            "The project is larger than 64 MB without dependencies and build output".into(),
        );
    }
    if output.exit_code != Some(0) {
        return Err(format!(
            "Could not pack the project: {}",
            one_line(&String::from_utf8_lossy(&output.stderr), 300)
        ));
    }
    Ok(output.stdout)
}

/// Up to `limit` bytes of a file, and whether it was longer.
pub async fn read_bytes(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    path: &str,
    limit: usize,
) -> Result<(Vec<u8>, bool), String> {
    let path = clean_path(path, &sandbox.slug)?;
    let script = in_project(sb, sandbox, "exec head -c \"$2\" -- \"$1\"");
    let count = (limit + 1).to_string();
    let argv = ["sh", "-c", script.as_str(), "sh", &path, &count];
    let mut output = tokio::time::timeout(
        Duration::from_secs(60),
        sb.exec_limited(sandbox, &argv, limit + 1),
    )
    .await
    .map_err(|_| "Reading the file took too long".to_string())?
    .map_err(|error| format!("Could not read the file: {error:#}"))?;
    if output.exit_code != Some(0) {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(one_line(&error, 200)
            .trim_start_matches("head: ")
            .to_owned());
    }
    let truncated = output.stdout.len() > limit;
    output.stdout.truncate(limit);
    Ok((output.stdout, truncated))
}

pub async fn read_text(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    path: &str,
    limit: usize,
) -> Result<(String, bool), String> {
    let path = clean_path(path, &sandbox.slug)?;
    let limit_arg = (limit + 1).to_string();
    let (out, err, code) = sh(
        sb,
        sandbox,
        "head -c \"$2\" -- \"$1\"",
        &[&path, &limit_arg],
        None,
        Duration::from_secs(20),
    )
    .await?;
    if code != Some(0) {
        return Err(one_line(&err, 200).trim_start_matches("head: ").to_owned());
    }
    let truncated = out.len() > limit;
    Ok((
        if truncated {
            out.chars().take(limit).collect()
        } else {
            out
        },
        truncated,
    ))
}

pub async fn list_entries(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    path: &str,
) -> Result<Vec<Value>, String> {
    let path = clean_path(path, &sandbox.slug)?;
    let script = "cd -- \"$1\" && find . -mindepth 1 -maxdepth 6 \\( -name node_modules -o -name .next -o -name .git \\) -prune -o \\( -type d -printf 'd %P\\n' \\) -o \\( -type f -printf 'f %s %P\\n' \\) | head -n 1000";
    let (out, err, code) = sh(sb, sandbox, script, &[&path], None, Duration::from_secs(20)).await?;
    if code != Some(0) {
        return Err(one_line(&err, 200));
    }
    let mut entries: Vec<Value> = out
        .lines()
        .filter_map(|line| {
            if let Some(dir) = line.strip_prefix("d ") {
                Some(json!({"path":dir,"type":"directory"}))
            } else {
                let rest = line.strip_prefix("f ")?;
                let (size, name) = rest.split_once(' ')?;
                Some(json!({"path":name,"type":"file","size":size.parse::<u64>().unwrap_or(0)}))
            }
        })
        .collect();
    entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(entries)
}

/// Short label for the activity list, available before the tool runs.
pub fn summary(name: &str, args: &Value) -> String {
    let path = || clean_path(arg(args, "path"), "").unwrap_or_default();
    match name {
        "list_files" => format!("Listed {}", path()),
        "read_file" => format!("Read {}", path()),
        "write_file" => format!("Wrote {}", path()),
        "edit_file" => format!("Edited {}", path()),
        "run_command" => format!("Ran {}", one_line(arg(args, "command"), 90)),
        "start_server" => format!(
            "Started the preview on port {}",
            args.get("port").and_then(Value::as_u64).unwrap_or(0)
        ),
        "server_logs" => "Checked the server logs".into(),
        "run_in_terminal" => format!(
            "Opened {} in the terminal",
            one_line(arg(args, "command"), 90)
        ),
        "plan" => {
            let n = args["files"].as_array().map_or(0, Vec::len);
            format!("Planned {n} file{}", if n == 1 { "" } else { "s" })
        }
        other => format!("Unknown tool {}", one_line(other, 40)),
    }
}

pub async fn run(sb: &Sandboxes, sandbox: &Sandbox, name: &str, raw_args: &str) -> Outcome {
    let args: Value = match serde_json::from_str(raw_args.trim()) {
        Ok(v @ Value::Object(_)) => v,
        _ if raw_args.trim().is_empty() => json!({}),
        _ => {
            return Outcome {
                ok: false,
                summary: summary(name, &json!({})),
                output: "Error: the tool arguments were not a JSON object.".into(),
                detail: "Invalid arguments".into(),
                preview: None,
                extra: json!({}),
            };
        }
    };
    let summary = summary(name, &args);
    sb.touch(sandbox).await;
    // A full workspace still runs commands, so files can be deleted.
    let full = sb.full(sandbox);
    if full && matches!(name, "write_file" | "edit_file") {
        let error = full_message(sb, sandbox);
        return Outcome {
            ok: false,
            summary,
            output: format!("Error: {error}"),
            detail: "Workspace full".into(),
            preview: None,
            extra: json!({}),
        };
    }
    let result = match name {
        "list_files" => list(sb, sandbox, &args).await,
        "read_file" => read_text(sb, sandbox, arg(&args, "path"), 2 * 1024 * 1024)
            .await
            .map(|(text, truncated)| {
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1);
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(2000);
                let (window, shown) = numbered(&text, offset as usize, limit as usize, truncated);
                (window, shown, None)
            }),
        "write_file" => {
            let append = args.get("append").and_then(Value::as_bool) == Some(true);
            write(sb, sandbox, arg(&args, "path"), arg(&args, "content"), append).await
        }
        "edit_file" => edit(sb, sandbox, &args).await,
        "run_command" => command(sb, sandbox, &args).await,
        "start_server" => start_server(sb, sandbox, &args).await,
        "server_logs" => logs(sb, sandbox, &args).await,
        "run_in_terminal" => terminal_command(&args).map(|_| {
            (
                "The command is typed into the person's Terminal tab, where they can use it with the keyboard. You cannot see what it shows: ask them if you need to know.".into(),
                String::new(),
                None,
            )
        }),
        "plan" => Ok((
            "Plan noted. Call plan again if it changes.".into(),
            String::new(),
            None,
        )),
        _ => Err(format!("There is no tool named {name}.")),
    };
    let result = match (name, result) {
        ("run_command", Ok((mut output, detail, preview))) if full => {
            match sb.measure(sandbox).await {
                Some(used) if used as u64 <= sb.config().disk_limit => {
                    output.push_str("\nThe workspace is no longer full.")
                }
                _ => {
                    output.push('\n');
                    output.push_str(&full_message(sb, sandbox));
                }
            }
            Ok((output, detail, preview))
        }
        ("write_file" | "edit_file" | "run_command", Ok((mut output, mut detail, mut preview))) => {
            follow_up(
                sb,
                sandbox,
                name,
                arg(&args, "path"),
                &mut output,
                &mut detail,
                &mut preview,
            )
            .await;
            Ok((output, detail, preview))
        }
        (_, result) => result,
    };
    match result {
        Ok((output, detail, preview)) => Outcome {
            ok: true,
            summary,
            extra: facts(name, &args, &sandbox.slug, &detail),
            output: shorten(&output, MODEL_OUTPUT),
            detail: shorten(&detail, 4096),
            preview,
        },
        Err(error) => Outcome {
            ok: false,
            summary,
            extra: facts(name, &args, &sandbox.slug, ""),
            output: format!("Error: {}", shorten(&error, MODEL_OUTPUT)),
            detail: shorten(&error, 4096),
            preview: None,
        },
    }
}

/// The one-line command the person's terminal runs for run_in_terminal.
fn terminal_command(args: &Value) -> Result<String, String> {
    let command = arg(args, "command").trim();
    if command.is_empty() || command.chars().count() > 1000 || command.chars().any(char::is_control)
    {
        return Err("Give one line of command, up to 1,000 characters.".into());
    }
    Ok(command.to_owned())
}

/// What the chat shows about a step besides its summary.
fn facts(name: &str, args: &Value, slug: &str, detail: &str) -> Value {
    let lines = |text: &str| text.lines().count();
    let path = clean_path(arg(args, "path"), slug).unwrap_or_default();
    let mut facts = match name {
        "write_file" => json!({"path": path, "added": lines(arg(args, "content"))}),
        "edit_file" => {
            json!({"path": path, "added": lines(arg(args, "new_text")), "removed": lines(arg(args, "old_text")).max(1)})
        }
        "run_in_terminal" => {
            terminal_command(args).map_or(json!({}), |command| json!({"command": command}))
        }
        "plan" => json!({
            "files": args["files"].as_array().into_iter().flatten().filter_map(Value::as_str)
                .filter_map(|f| clean_path(f, slug).ok()).take(60).collect::<Vec<_>>(),
        }),
        _ => json!({}),
    };
    if detail.ends_with(" · preview OK") || detail.ends_with(" · syntax OK") {
        facts["check"] = json!("ok");
    } else if detail.ends_with(" · preview error") || detail.ends_with(" · syntax error") {
        facts["check"] = json!("error");
    }
    // A plain web server has no live reload, so the preview pane reloads itself.
    if [" · preview reloaded", " · syntax OK", " · syntax error"]
        .iter()
        .any(|end| detail.ends_with(end))
    {
        facts["reload"] = json!(true);
    }
    facts
}

type ToolResult = Result<(String, String, Option<u16>), String>;

fn full_message(sb: &Sandboxes, sandbox: &Sandbox) -> String {
    let gib = |bytes: f64| bytes / f64::from(1u32 << 30);
    format!(
        "The workspace is full ({:.1} of {:.1} GiB, shared by all of the user's workspaces), so files cannot be written. Free space with run_command (for example remove node_modules, build output or large data files) or ask the user to delete old workspaces.",
        gib(sandbox.disk_bytes.unwrap_or(0) as f64),
        gib(sb.config().disk_limit as f64)
    )
}

async fn list(sb: &Sandboxes, sandbox: &Sandbox, args: &Value) -> ToolResult {
    let entries = list_entries(sb, sandbox, arg(args, "path")).await?;
    let text = entries
        .iter()
        .take(400)
        .map(|e| match e["type"].as_str() {
            Some("directory") => format!("{}/", e["path"].as_str().unwrap_or("")),
            _ => format!("{} ({} bytes)", e["path"].as_str().unwrap_or(""), e["size"]),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let detail = format!("{} entries", entries.len());
    Ok((
        if text.is_empty() {
            "(empty)".into()
        } else {
            text
        },
        detail,
        None,
    ))
}

async fn write(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    path: &str,
    content: &str,
    append: bool,
) -> ToolResult {
    let path = clean_path(path, &sandbox.slug)?;
    if content.len() > 1024 * 1024 {
        return Err("Files are limited to 1 MB".into());
    }
    // Appending keeps the whole file under the same limit.
    let script = if append {
        "mkdir -p -- \"$(dirname -- \"$1\")\" && cat >> \"$1\" && [ \"$(wc -c < \"$1\")\" -le 1048576 ] || { echo 'Files are limited to 1 MB' >&2; exit 1; }"
    } else {
        "mkdir -p -- \"$(dirname -- \"$1\")\" && cat > \"$1\""
    };
    let (_, err, code) = sh(
        sb,
        sandbox,
        script,
        &[&path],
        Some(content.as_bytes()),
        Duration::from_secs(30),
    )
    .await?;
    if code != Some(0) {
        return Err(one_line(&err, 300));
    }
    let lines = content.lines().count();
    Ok((
        format!(
            "{} {path} ({} bytes).",
            if append { "Added to" } else { "Wrote" },
            content.len()
        ),
        format!("{lines} lines"),
        None,
    ))
}

async fn edit(sb: &Sandboxes, sandbox: &Sandbox, args: &Value) -> ToolResult {
    let (old, new) = (arg(args, "old_text"), arg(args, "new_text"));
    if old.is_empty() {
        return Err("old_text must not be empty. To create a file, use write_file.".into());
    }
    if old == new {
        return Err("old_text and new_text are the same, so there is nothing to change.".into());
    }
    let (text, truncated) = read_text(sb, sandbox, arg(args, "path"), 1024 * 1024).await?;
    if truncated {
        return Err("The file is larger than 1 MB; use write_file instead".into());
    }
    let all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let updated = replace(&text, old, new, all)?;
    write(sb, sandbox, arg(args, "path"), &updated, false).await?;
    let path = clean_path(arg(args, "path"), &sandbox.slug)?;
    let detail = format!(
        "-{} +{} lines",
        old.lines().count().max(1),
        new.lines().count()
    );
    Ok((format!("Edited {path}."), detail, None))
}

/// Shows a window of a file with line numbers, as much as fits in one tool result.
fn numbered(text: &str, offset: usize, limit: usize, truncated: bool) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return ("(The file is empty.)".into(), "empty".into());
    }
    let first = offset.max(1);
    if first > total {
        return (
            format!("The file has only {total} lines; use a smaller offset."),
            format!("{total} lines"),
        );
    }
    let budget = MODEL_OUTPUT - 300;
    let mut out = String::new();
    let mut last = first - 1;
    for (i, line) in lines.iter().enumerate().skip(first - 1).take(limit) {
        let line: String = if line.chars().count() > 2000 {
            format!(
                "{}… (line shortened)",
                line.chars().take(2000).collect::<String>()
            )
        } else {
            (*line).to_owned()
        };
        let entry = format!("{}: {line}\n", i + 1);
        if out.len() + entry.len() > budget && last >= first {
            break;
        }
        out.push_str(&entry);
        last = i + 1;
    }
    if last < total || truncated {
        out.push_str(&format!(
            "(Showing lines {first}-{last} of {}{}. Use offset={} to read on.)",
            total,
            if truncated { "+" } else { "" },
            last + 1
        ));
    } else {
        out.push_str(&format!("(End of file, {total} lines.)"));
    }
    (out, format!("lines {first}-{last} of {total}"))
}

/// Replaces old with new: an exact match first, then a match that ignores the
/// line-number prefixes read_file adds, then one that ignores indentation and
/// trailing spaces line by line. Unless all is set the match must be unique.
fn replace(text: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    let unique = |count: usize| -> Result<(), String> {
        match count {
            0 => Err(String::new()),
            1 => Ok(()),
            _ if all => Ok(()),
            n => Err(format!("old_text appears {n} times. Include more surrounding lines so it is unique, or set replace_all to change all of them.")),
        }
    };
    let exact = text.matches(old).count();
    match unique(exact) {
        Ok(()) => {
            return Ok(if all {
                text.replace(old, new)
            } else {
                text.replacen(old, new, 1)
            })
        }
        Err(error) if !error.is_empty() => return Err(error),
        Err(_) => {}
    }
    let (old, new) = (unnumbered(old), unnumbered(new));
    if text.contains(old.as_str()) {
        return replace(text, &old, &new, all);
    }
    // Line by line, ignoring leading and trailing whitespace.
    let wanted: Vec<&str> = old.trim_matches('\n').lines().map(str::trim).collect();
    if wanted.iter().all(|line| line.is_empty()) {
        return Err(not_found());
    }
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let lines: Vec<&str> = text.split('\n').collect();
    let spans: Vec<(usize, usize)> = (0..lines.len().saturating_sub(wanted.len() - 1))
        .filter(|&i| {
            wanted
                .iter()
                .enumerate()
                .all(|(j, want)| lines[i + j].trim() == *want)
        })
        .map(|i| {
            let end = starts[i + wanted.len() - 1] + lines[i + wanted.len() - 1].len();
            (starts[i], end)
        })
        .collect();
    unique(spans.len()).map_err(|error| if error.is_empty() { not_found() } else { error })?;
    let new = new.trim_matches('\n');
    let mut out = text.to_owned();
    for &(start, end) in spans.iter().rev().take(if all { usize::MAX } else { 1 }) {
        out.replace_range(start..end, new);
    }
    Ok(out)
}

fn not_found() -> String {
    "old_text was not found in the file. Read the file again with read_file and copy the lines exactly, without the line-number prefixes.".into()
}

/// Removes `12: ` prefixes when every non-empty line has one.
fn unnumbered(text: &str) -> String {
    let strip = |line: &str| -> Option<String> {
        let (number, rest) = line
            .split_once(": ")
            .or_else(|| line.split_once(':').filter(|(_, r)| r.is_empty()))?;
        (!number.trim().is_empty() && number.trim().chars().all(|c| c.is_ascii_digit()))
            .then(|| rest.to_owned())
    };
    let stripped: Option<Vec<String>> = text
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                Some(line.to_owned())
            } else {
                strip(line)
            }
        })
        .collect();
    match stripped {
        Some(lines) if text.lines().any(|line| !line.trim().is_empty()) => {
            let mut out = lines.join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            out
        }
        _ => text.to_owned(),
    }
}

/// Recognises a plain dev server start (`npm run dev …`, `next dev …`) so it runs in
/// the background instead of being killed by the command timeout. Returns the
/// command and its port when it names one (-p, --port or PORT=).
fn dev_server(script: &str) -> Option<(String, Option<u16>)> {
    let mut command = script.trim();
    // `cd /workspace…` first is a no-op: commands already run in the project folder.
    if let Some(rest) = command.strip_prefix("cd /workspace") {
        if let Some(at) = rest.find(['&', ';']) {
            if !rest[..at].trim_end().contains(' ') {
                command = rest[at..].trim_start_matches(['&', ';']).trim();
            }
        }
    }
    let command = command.strip_suffix('&').unwrap_or(command).trim();
    if command.contains(['&', ';', '|', '>', '<', '`', '$', '\n']) {
        return None;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    let start = words.iter().position(|w| !w.contains('='))?;
    let program = words[start..].join(" ");
    const STARTS: [&str; 7] = [
        "npm run dev",
        "npm start",
        "npm run start",
        "npx next dev",
        "npx next start",
        "next dev",
        "next start",
    ];
    if !STARTS
        .iter()
        .any(|s| program == *s || program.starts_with(&format!("{s} ")))
    {
        return None;
    }
    let mut port = None;
    for (i, word) in words.iter().enumerate() {
        let value = match *word {
            "-p" | "--port" => words.get(i + 1).copied(),
            w => w
                .strip_prefix("--port=")
                .or_else(|| w.strip_prefix("PORT=")),
        };
        if let Some(p) = value.and_then(|v| v.parse::<u16>().ok()) {
            port = Some(p);
        }
    }
    Some((command.to_owned(), port))
}

const TOOLS: [&str; 7] = [
    "list_files",
    "read_file",
    "write_file",
    "edit_file",
    "run_command",
    "start_server",
    "server_logs",
];

/// A tool written as a shell command, such as `start_server command="./app" port=3000`,
/// with its key=value (or JSON) arguments.
fn tool_as_command(script: &str) -> Option<(&'static str, Value)> {
    let script = script.trim();
    let name = script.split_whitespace().next()?;
    let tool = TOOLS.into_iter().find(|t| *t == name)?;
    let rest = script[name.len()..].trim();
    if let Ok(value @ Value::Object(_)) = serde_json::from_str(rest) {
        return Some((tool, value));
    }
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    for c in rest.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    words.push(word);
    let mut call = json!({});
    for word in words {
        if let Some((key, value)) = word.split_once('=') {
            let key = key.trim_start_matches('-');
            call[key] = match value.parse::<u64>() {
                Ok(n) => json!(n),
                Err(_) => json!(value),
            };
        }
    }
    Some((tool, call))
}

async fn command(sb: &Sandboxes, sandbox: &Sandbox, args: &Value) -> ToolResult {
    let script = arg(args, "command");
    if script.trim().is_empty() || script.len() > 16 * 1024 {
        return Err("The command must be between 1 and 16384 characters".into());
    }
    let words = || script.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
    if words().any(|w| w == "pkill" || w == "killall") {
        return Err("pkill and killall are not available because they can stop the workspace shell itself. Chat manages the dev server; call start_server to restart a server.".into());
    }
    if let Some((tool, mut call)) = tool_as_command(script) {
        let note = format!("{tool} is one of your tools, not a shell command");
        if tool != "start_server" || !call["command"].is_string() {
            return Err(format!("{note}. Call the {tool} tool directly."));
        }
        if call.get("port").is_none() {
            call["port"] = json!(sandbox.port);
        }
        let note = format!("{note}; it was called as a tool. Call it directly next time.");
        return match start_server(sb, sandbox, &call).await {
            Ok((output, detail, preview)) => Ok((format!("{note}\n{output}"), detail, preview)),
            Err(error) => Err(format!("{note}\n{error}")),
        };
    }
    if let Some((server, port)) = dev_server(script) {
        let port = port.unwrap_or(sandbox.port as u16);
        let note =
            "This is a long-running server, so it was started in the background with start_server.";
        return match start_server(sb, sandbox, &json!({"command": server, "port": port})).await {
            Ok((output, detail, preview)) => Ok((format!("{note}\n{output}"), detail, preview)),
            Err(error) => Err(format!("{note}\n{error}")),
        };
    }
    let seconds = args
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(120)
        .clamp(1, 300);
    let started = Instant::now();
    let limit = seconds.to_string();
    let (out, err, code) = sh(
        sb,
        sandbox,
        "timeout -k 5 \"$1\" sh -c \"$2\"",
        &[&limit, script],
        None,
        Duration::from_secs(seconds + 15),
    )
    .await?;
    let code = code.unwrap_or(-1);
    let elapsed = started.elapsed().as_secs_f32();
    let mut output = format!(
        "exit code {code}{}\n",
        if code == 124 { " (timed out)" } else { "" }
    );
    if !out.is_empty() {
        output.push_str(&format!("stdout:\n{}\n", shorten(&out, 8 * 1024)));
    }
    if !err.is_empty() {
        output.push_str(&format!("stderr:\n{}\n", shorten(&err, 4 * 1024)));
    }
    let tail = format!("{}{}", out, err);
    let detail = format!(
        "exit {code} · {elapsed:.1}s\n{}",
        shorten(tail.trim_end(), 2000)
    );
    if code == 0 {
        Ok((output, detail, None))
    } else {
        Err(output)
    }
}

async fn start_server(sb: &Sandboxes, sandbox: &Sandbox, args: &Value) -> ToolResult {
    let script = arg(args, "command");
    let port = args
        .get("port")
        .and_then(Value::as_u64)
        .filter(|p| (1024..=65535).contains(p))
        .ok_or("port must be between 1024 and 65535")? as u16;
    if script.trim().is_empty() || script.len() > 4096 {
        return Err("The command must be between 1 and 4096 characters".into());
    }
    // A new process group per server so a restart stops the whole tree.
    let launcher = format!(
        "if [ -s {SERVER_PID} ]; then kill -TERM -\"$(cat {SERVER_PID})\" 2>/dev/null; sleep 1; fi; \
         echo \"$2\" > {SERVER_PORT}; echo \"${{3:-custom}}\" > {SERVER_KIND}; \
         setsid nohup sh -c \"$1\" > {SERVER_LOG} 2>&1 < /dev/null & echo $! > {SERVER_PID}"
    );
    let port_arg = port.to_string();
    let (_, err, code) = sh(
        sb,
        sandbox,
        &launcher,
        &[script, &port_arg, arg(args, "kind")],
        None,
        Duration::from_secs(30),
    )
    .await?;
    if code != Some(0) {
        return Err(format!(
            "The server could not start: {}",
            one_line(&err, 300)
        ));
    }
    let Some(ip) = sandbox.pod_ip.as_deref() else {
        return Err("The workspace has no network address yet".into());
    };
    let url = format!("http://{ip}:{port}/");
    let started = Instant::now();
    let mut ready = false;
    let mut polls = 0;
    while started.elapsed() < Duration::from_secs(90) {
        if sb
            .http()
            .get(&url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        polls += 1;
        // Stop waiting as soon as the server process has exited.
        if polls % 4 == 0 {
            let check = format!("kill -0 \"$(cat {SERVER_PID})\" 2>/dev/null");
            let alive = sh(sb, sandbox, &check, &[], None, Duration::from_secs(10)).await;
            if matches!(alive, Ok((_, _, Some(code))) if code != 0) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
    let (log, _, _) = sh(
        sb,
        sandbox,
        &format!("tail -n 30 {SERVER_LOG}"),
        &[],
        None,
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_default();
    if !ready {
        return Err(format!(
            "Nothing answered on port {port}. Server output:\n{}",
            shorten(&log, 4096)
        ));
    }
    sb.set_preview(sandbox, Some(port)).await.map_err(|e| e.1)?;
    Ok((
        format!("The server is running on port {port} and the user can see it in the preview pane. Recent output:\n{}", shorten(&log, 3000)),
        format!("Port {port} · ready in {:.0}s", started.elapsed().as_secs_f32()),
        Some(port),
    ))
}

/// After a change: start the dev server if the workspace now holds a runnable app,
/// and after a file edit report whether the affected page still compiles.
async fn follow_up(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    name: &str,
    path: &str,
    output: &mut String,
    detail: &mut String,
    preview: &mut Option<u16>,
) {
    let server = match ensure_dev_server(sb, sandbox).await {
        Ok(Some(server)) => {
            if server.started {
                output.push_str(&format!("\nChat started the {} on port {}; the user now sees the app in the preview pane.", if server.kind == "static" { "web server" } else { "dev server" }, server.port));
                *preview = Some(server.port);
            }
            server
        }
        Ok(None) => return,
        Err(error) => {
            output.push_str(&format!("\nChat could not start the dev server: {error}"));
            detail.push_str(" · server failed");
            return;
        }
    };
    let path = clean_path(path, &sandbox.slug).unwrap_or_default();
    if name == "run_command" || !checked(&path) {
        return;
    }
    if server.kind == "static" {
        // Plain files have no build step: the preview reloads, and scripts get a syntax check.
        let (text, note) = syntax_check(sb, sandbox, &path).await;
        output.push('\n');
        output.push_str(&text);
        detail.push_str(note);
        return;
    }
    let (text, note) = compile_check(sb, sandbox, server.port, &path).await;
    output.push('\n');
    output.push_str(&text);
    detail.push_str(note);
}

/// The project's web server.
pub struct DevServer {
    pub port: u16,
    /// True when this call started it.
    pub started: bool,
    /// "next", "vite", "static" (Chat serving plain files) or "custom" (start_server).
    pub kind: String,
}

/// Returns the running server, starting one when none is running: the Next.js or Vite
/// dev server when the workspace has one installed, or a plain web server for a
/// project with an index.html and no build step.
pub async fn ensure_dev_server(
    sb: &Sandboxes,
    sandbox: &Sandbox,
) -> Result<Option<DevServer>, String> {
    let probe = format!(
        "pid=$(cat {SERVER_PID} 2>/dev/null); \
         if [ -n \"$pid\" ] && kill -0 \"$pid\" 2>/dev/null; then echo \"running $(cat {SERVER_PORT} 2>/dev/null) $(cat {SERVER_KIND} 2>/dev/null)\"; exit 0; fi; \
         if [ -f package.json ] && grep -q '\"dev\"[[:space:]]*:' package.json; then \
           if [ -x node_modules/.bin/next ]; then echo next; elif [ -x node_modules/.bin/vite ]; then echo vite; else echo none; fi; \
         elif ls ./*.html >/dev/null 2>&1; then echo static; else echo none; fi"
    );
    let (out, _, _) = sh(sb, sandbox, &probe, &[], None, Duration::from_secs(15)).await?;
    let port = sandbox.port as u16;
    let mut words = out.split_whitespace();
    let kind = words.next().unwrap_or("none");
    let command = match kind {
        "running" => {
            let port = words
                .next()
                .and_then(|p| p.parse().ok())
                .or(sandbox.preview_port.and_then(|p| u16::try_from(p).ok()))
                .unwrap_or(port);
            let kind = words.next().unwrap_or("custom").into();
            return Ok(Some(DevServer {
                port,
                started: false,
                kind,
            }));
        }
        "next" => format!("npm run dev -- -H 0.0.0.0 -p {port}"),
        "vite" => format!("npm run dev -- --host 0.0.0.0 --port {port} --strictPort"),
        "static" => format!("{STATIC_SERVER} {port}"),
        _ => return Ok(None),
    };
    start_server(
        sb,
        sandbox,
        &json!({"command": command, "port": port, "kind": kind}),
    )
    .await?;
    Ok(Some(DevServer {
        port,
        started: true,
        kind: kind.into(),
    }))
}

/// Files whose edits can break the running app.
fn checked(path: &str) -> bool {
    let path = path
        .trim_start_matches("/workspace/")
        .trim_start_matches("./");
    !path.starts_with("node_modules/")
        && !path.starts_with(".next/")
        && [
            ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".css", ".json", ".vue", ".svelte",
            ".html",
        ]
        .iter()
        .any(|ext| path.ends_with(ext))
}

/// The page an edit most likely affects: app/<route>/page.* maps to /<route>,
/// everything else (layouts, styles, components) to the home page.
fn route_for(path: &str) -> String {
    let path = path
        .trim_start_matches("/workspace/")
        .trim_start_matches("./");
    let Some(rest) = path
        .strip_prefix("app/")
        .or_else(|| path.strip_prefix("src/app/"))
    else {
        return "/".into();
    };
    let mut parts: Vec<&str> = rest.split('/').collect();
    if !parts.pop().is_some_and(|file| file.starts_with("page.")) {
        return "/".into();
    }
    let mut route = String::new();
    for part in parts {
        if part.starts_with('(') && part.ends_with(')') {
            continue;
        }
        if part.starts_with(['[', '@', '_']) {
            return "/".into();
        }
        route.push('/');
        route.push_str(part);
    }
    if route.is_empty() {
        "/".into()
    } else {
        route
    }
}

/// For Vite, the URL of the edited module itself (Vite compiles modules on request).
fn vite_route(path: &str) -> String {
    let path = path
        .trim_start_matches("/workspace/")
        .trim_start_matches("./");
    let module = [
        ".tsx", ".ts", ".jsx", ".js", ".mjs", ".css", ".vue", ".svelte",
    ]
    .iter()
    .any(|ext| path.ends_with(ext));
    let config = path.contains(".config.") || !path.contains('/');
    if module && !config {
        format!("/{path}")
    } else {
        "/".into()
    }
}

/// For a project without a build step: checks a script's syntax with Node.
async fn syntax_check(sb: &Sandboxes, sandbox: &Sandbox, path: &str) -> (String, &'static str) {
    if ![".js", ".mjs"].iter().any(|ext| path.ends_with(ext)) {
        return (
            "The preview reloads to show the change.".into(),
            " · preview reloaded",
        );
    }
    match sh(
        sb,
        sandbox,
        "node --check \"$1\" 2>&1",
        &[path],
        None,
        Duration::from_secs(20),
    )
    .await
    {
        Ok((_, _, Some(0))) => (
            format!("{path} has no syntax errors; the preview reloads to show the change."),
            " · syntax OK",
        ),
        Ok((out, err, _)) => (
            format!(
                "{path} has a syntax error, so the page will not run:\n{}\nFix this before replying.",
                shorten(format!("{out}{err}").trim(), 3000)
            ),
            " · syntax error",
        ),
        Err(_) => (String::new(), ""),
    }
}

/// Requests a page from the dev server, which compiles it, and reports the result
/// with any new server output.
async fn compile_check(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    port: u16,
    path: &str,
) -> (String, &'static str) {
    let Some(ip) = sandbox.pod_ip.as_deref() else {
        return (String::new(), "");
    };
    let probe = format!(
        "echo \"$(stat -c %s {SERVER_LOG} 2>/dev/null || echo 0) \
         $([ -x node_modules/.bin/vite ] && [ ! -x node_modules/.bin/next ] && echo vite)\""
    );
    let probe = sh(sb, sandbox, &probe, &[], None, Duration::from_secs(10))
        .await
        .map(|(out, _, _)| out)
        .unwrap_or_default();
    let mut probe = probe.split_whitespace();
    let before = probe
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    // Vite serves the page even when a module is broken, so request the module.
    let route = if probe.next() == Some("vite") {
        vite_route(path)
    } else {
        route_for(path)
    };
    // Give the file watcher a moment to see the change.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let response = sb
        .http()
        .get(format!("http://{ip}:{port}{route}"))
        .timeout(Duration::from_secs(60))
        .send()
        .await;
    let start = (before + 1).to_string();
    let fresh = sh(
        sb,
        sandbox,
        &format!("tail -c +\"$1\" {SERVER_LOG} 2>/dev/null | tail -n 40"),
        &[&start],
        None,
        Duration::from_secs(10),
    )
    .await
    .map(|(out, _, _)| strip_ansi(&out))
    .unwrap_or_default();
    let fresh = fresh.trim();
    match response {
        Ok(response) if response.status().is_server_error() => {
            let status = response.status().as_u16();
            let output = if fresh.is_empty() {
                strip_ansi(&response.text().await.unwrap_or_default())
            } else {
                fresh.to_owned()
            };
            (
                format!(
                    "The preview is broken: {route} returned HTTP {status}. Error output:\n{}\nFix this before replying.",
                    shorten(output.trim(), 4096)
                ),
                " · preview error",
            )
        }
        Ok(response) => {
            let status = response.status().as_u16();
            let mut text = format!("The preview compiles: {route} returned HTTP {status}.");
            if fresh.contains("rror") || fresh.contains('⨯') {
                text.push_str(&format!(
                    " New dev server output:\n{}",
                    shorten(fresh, 3000)
                ));
            }
            (text, if status < 400 { " · preview OK" } else { "" })
        }
        Err(_) => (
            format!("The dev server did not answer at {route}. Check server_logs."),
            " · no answer",
        ),
    }
}

/// Checkpoints live in a git directory outside the project folder, so a project's own
/// repository is never touched and downloads do not include them.
const CHECKPOINTS: &str = "export GIT_DIR=/workspace/.obleth-checkpoints/$OBLETH_SLUG.git GIT_WORK_TREE=.; \
    if [ ! -d \"$GIT_DIR\" ]; then git init -q \"$GIT_DIR\" && printf 'node_modules/\\n.next/\\ndist/\\nbuild/\\n.git/\\n.venv/\\n__pycache__/\\n' > \"$GIT_DIR/info/exclude\" || exit 1; fi; \
    git() { command git -c user.name=obleth -c user.email=checkpoints@obleth.invalid -c core.quotepath=off \"$@\"; }";

/// Saves the project as it is now. None when nothing changed since the last one.
pub async fn checkpoint(
    sb: &Sandboxes,
    sandbox: &Sandbox,
    label: &str,
) -> Option<(String, String)> {
    let script = format!(
        "{CHECKPOINTS}\ngit add -A >/dev/null 2>&1 && git commit -q -m \"$1\" >/dev/null 2>&1 || exit 3\n\
         git rev-parse --short=12 HEAD && git show --shortstat --format= HEAD"
    );
    match sh(
        sb,
        sandbox,
        &script,
        &[label],
        None,
        Duration::from_secs(30),
    )
    .await
    {
        Ok((out, _, Some(0))) => {
            let mut lines = out.lines();
            let sha = lines.next()?.trim().to_owned();
            let stat = lines
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_owned();
            Some((sha, stat))
        }
        _ => None,
    }
}

/// Puts the project back as it was at a checkpoint: changed files are restored and
/// files added since are removed. Ignored folders such as node_modules stay. The
/// current state is saved first; its id is returned so the restore can be undone.
pub async fn restore(sb: &Sandboxes, sandbox: &Sandbox, sha: &str) -> Result<String, String> {
    if !(7..=40).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Unknown checkpoint".into());
    }
    let script = format!(
        "{CHECKPOINTS}\ngit cat-file -e \"$1^{{commit}}\" 2>/dev/null || exit 4\n\
         git add -A >/dev/null 2>&1; git commit -q -m \"Before restoring $1\" >/dev/null 2>&1; \
         before=$(git rev-parse --short=12 HEAD) && git read-tree -u --reset \"$1\" && \
         {{ git commit -q -m \"Restored $1\" >/dev/null 2>&1; echo \"$before\"; }}"
    );
    match sh(sb, sandbox, &script, &[sha], None, Duration::from_secs(60)).await? {
        (out, _, Some(0)) => Ok(out.trim().to_owned()),
        (_, _, Some(4)) => Err("Unknown checkpoint".into()),
        (_, err, _) => Err(format!(
            "The checkpoint could not be restored: {}",
            one_line(&err, 200)
        )),
    }
}

/// Removes terminal colour codes from server output.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Stops the project's server, if any.
pub async fn stop_server(sb: &Sandboxes, sandbox: &Sandbox) {
    let script = format!(
        "pid=$(cat {SERVER_PID} 2>/dev/null); [ -n \"$pid\" ] && kill -TERM -\"$pid\" 2>/dev/null; rm -f {SERVER_PID} {SERVER_PORT} {SERVER_KIND}"
    );
    if let Err(error) = sh(sb, sandbox, &script, &[], None, Duration::from_secs(15)).await {
        tracing::warn!("could not stop a workspace server: {error}");
    }
}

async fn logs(sb: &Sandboxes, sandbox: &Sandbox, args: &Value) -> ToolResult {
    let lines = args
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(80)
        .clamp(1, 400)
        .to_string();
    let (out, _, code) = sh(
        sb,
        sandbox,
        &format!("tail -n \"$1\" {SERVER_LOG}"),
        &[&lines],
        None,
        Duration::from_secs(15),
    )
    .await?;
    if code != Some(0) {
        return Err("No server has been started yet".into());
    }
    Ok((
        if out.is_empty() {
            "(no output)".into()
        } else {
            out.clone()
        },
        format!("{} lines", out.lines().count()),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_map_to_the_page_they_affect() {
        assert_eq!(route_for("app/page.tsx"), "/");
        assert_eq!(route_for("/workspace/app/about/page.tsx"), "/about");
        assert_eq!(route_for("src/app/(site)/blog/page.jsx"), "/blog");
        assert_eq!(route_for("app/posts/[id]/page.tsx"), "/");
        assert_eq!(route_for("app/layout.tsx"), "/");
        assert_eq!(route_for("components/Hero.tsx"), "/");
        assert_eq!(vite_route("src/App.tsx"), "/src/App.tsx");
        assert_eq!(
            vite_route("/workspace/src/styles/site.css"),
            "/src/styles/site.css"
        );
        assert_eq!(vite_route("vite.config.ts"), "/");
        assert_eq!(vite_route("index.html"), "/");
        assert!(checked("app/globals.css"));
        assert!(!checked("README.md"));
        assert!(!checked("node_modules/x/index.js"));
    }

    #[test]
    fn tools_written_as_commands_are_recognized() {
        let (tool, call) = tool_as_command(
            r#"start_server command="./target/release/pi-calculator --fast" port=3002"#,
        )
        .unwrap();
        assert_eq!(tool, "start_server");
        assert_eq!(
            call,
            json!({"command":"./target/release/pi-calculator --fast","port":3002})
        );
        assert_eq!(
            tool_as_command(r#"start_server {"command":"python3 app.py","port":3001}"#)
                .unwrap()
                .1,
            json!({"command":"python3 app.py","port":3001})
        );
        assert_eq!(tool_as_command("server_logs").unwrap().0, "server_logs");
        assert!(tool_as_command("start_servers.sh").is_none());
        assert!(tool_as_command("cargo build && ./start_server").is_none());
    }

    #[test]
    fn system_prompt_names_the_project_folder_and_port() {
        let now = chrono::Utc::now();
        let sandbox = Sandbox {
            id: uuid::Uuid::nil(),
            conversation_id: uuid::Uuid::nil(),
            owner_id: uuid::Uuid::nil(),
            slug: "abcdefgh12345678".into(),
            pod_id: uuid::Uuid::nil(),
            pod_slug: "pppppppppppppppp".into(),
            status: "running".into(),
            pod_ip: None,
            port: 3004,
            preview_port: None,
            error_message: None,
            disk_bytes: None,
            created_at: now,
            last_used_at: now,
        };
        let prompt = system_prompt(&sandbox, None, "");
        assert!(prompt.contains("The project folder is /workspace/abcdefgh12345678;"));
        assert!(prompt.contains(DEFAULT_INSTRUCTIONS));
        let own = system_prompt(&sandbox, Some("Always use Svelte."), "# Right now");
        assert!(own.contains("Always use Svelte.") && !own.contains("# How to work"));
        assert!(own.contains("on port 3004") && own.ends_with("# Right now"));
        assert!(prompt.contains("on port 3004"));
        assert!(!prompt.contains('\\'), "{prompt}");
    }

    #[test]
    fn edits_tolerate_indentation_and_line_numbers() {
        let file = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";
        assert_eq!(
            replace(file, "let a = 1;", "let a = 3;", false).unwrap(),
            "fn main() {\n    let a = 3;\n    let b = 2;\n}\n"
        );
        // Wrong indentation still finds the lines.
        assert_eq!(
            replace(file, "  let a = 1;\n  let b = 2;", "    let c = 3;", false).unwrap(),
            "fn main() {\n    let c = 3;\n}\n"
        );
        // Text copied with read_file's line numbers.
        assert_eq!(
            replace(
                file,
                "2:     let a = 1;\n3:     let b = 2;",
                "2:     let a = 9;\n3:     let b = 2;",
                false
            )
            .unwrap(),
            "fn main() {\n    let a = 9;\n    let b = 2;\n}\n"
        );
        let twice = "x = 1\nx = 1\n";
        assert!(replace(twice, "x = 1", "x = 2", false)
            .unwrap_err()
            .contains("2 times"));
        assert_eq!(
            replace(twice, "x = 1", "x = 2", true).unwrap(),
            "x = 2\nx = 2\n"
        );
        assert!(replace(file, "let z = 0;", "", false)
            .unwrap_err()
            .contains("not found"));
    }

    #[test]
    fn reads_come_back_numbered_in_windows() {
        let (out, _) = numbered("a\nb\nc\n", 1, 2000, false);
        assert_eq!(out, "1: a\n2: b\n3: c\n(End of file, 3 lines.)");
        let (out, _) = numbered("a\nb\nc\n", 2, 1, false);
        assert_eq!(
            out,
            "2: b\n(Showing lines 2-2 of 3. Use offset=3 to read on.)"
        );
        let big = "x".repeat(100) + "\n";
        let (out, _) = numbered(&big.repeat(1000), 1, 2000, false);
        assert!(
            out.len() <= MODEL_OUTPUT && out.ends_with("to read on.)"),
            "{}",
            out.len()
        );
    }

    #[test]
    fn colour_codes_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[33m\u{1b}[1m>\u{1b}[0m 8 | x"), "> 8 | x");
    }

    #[test]
    fn dev_server_commands_are_recognised() {
        assert_eq!(
            dev_server("npm run dev -- -H 0.0.0.0 -p 3000"),
            Some(("npm run dev -- -H 0.0.0.0 -p 3000".into(), Some(3000)))
        );
        assert_eq!(
            dev_server("cd /workspace && npx next dev --port=4000 &"),
            Some(("npx next dev --port=4000".into(), Some(4000)))
        );
        assert_eq!(
            dev_server("PORT=5000 npm start"),
            Some(("PORT=5000 npm start".into(), Some(5000)))
        );
        assert_eq!(
            dev_server("npm run dev"),
            Some(("npm run dev".into(), None))
        );
        assert_eq!(
            dev_server("cd /workspace/abc && npm run dev"),
            Some(("npm run dev".into(), None))
        );
        assert_eq!(dev_server("cd /workspace/a b && npm run dev"), None);
        assert_eq!(dev_server("npm run build"), None);
        assert_eq!(dev_server("npm run dev > log.txt"), None);
        assert_eq!(dev_server("npm install && npm run dev"), None);
        assert_eq!(dev_server("npm run development"), None);
    }

    #[test]
    fn paths_resolve_inside_the_workspace_root() {
        assert_eq!(
            clean_path("/workspace/app/page.tsx", "").unwrap(),
            "app/page.tsx"
        );
        assert_eq!(clean_path("", "").unwrap(), ".");
        let slug = "abcdefgh12345678";
        assert_eq!(
            clean_path("/workspace/abcdefgh12345678/src/App.tsx", slug).unwrap(),
            "src/App.tsx"
        );
        assert_eq!(
            clean_path("/workspace/src/App.tsx", slug).unwrap(),
            "src/App.tsx"
        );
        assert_eq!(
            clean_path("/workspace/abcdefgh12345678x/a", slug).unwrap(),
            "abcdefgh12345678x/a"
        );
        assert_eq!(clean_path("/workspace", "").unwrap(), ".");
        assert!(clean_path("a\nb", "").is_err());
        for outside in [
            "..",
            "../abcdefgh87654321/src/App.tsx",
            "src/../../other",
            "/workspace/abcdefgh12345678/../x",
            "/workspace/..",
        ] {
            assert!(clean_path(outside, slug).is_err(), "{outside}");
        }
        assert_eq!(clean_path("a..b/..c", slug).unwrap(), "a..b/..c");
    }

    #[test]
    fn long_output_keeps_the_end_on_a_character_boundary() {
        let text = format!("{}é tail", "x".repeat(20));
        let short = shorten(&text, 7);
        assert!(short.ends_with("é tail") || short.ends_with(" tail"));
        assert!(short.starts_with('['));
    }

    #[test]
    fn terminal_commands_are_one_line() {
        assert_eq!(
            terminal_command(&json!({"command":"  cargo run --release "})).unwrap(),
            "cargo run --release"
        );
        assert!(terminal_command(&json!({"command":"cargo run\nrm -rf ~"})).is_err());
        assert!(terminal_command(&json!({"command":""})).is_err());
        assert_eq!(
            facts(
                "run_in_terminal",
                &json!({"command":"python app.py"}),
                "abc",
                ""
            ),
            json!({"command":"python app.py"})
        );
        assert_eq!(
            summary("run_in_terminal", &json!({"command":"cargo run"})),
            "Opened cargo run in the terminal"
        );
    }

    #[test]
    fn summaries_describe_each_tool() {
        assert_eq!(
            summary("write_file", &json!({"path":"/workspace/app/page.tsx"})),
            "Wrote app/page.tsx"
        );
        assert_eq!(
            summary("run_command", &json!({"command":"npm run build\nmore"})),
            "Ran npm run build"
        );
        assert_eq!(
            summary("plan", &json!({"files":["a.js","b.js"]})),
            "Planned 2 files"
        );
        assert!(definitions().as_array().unwrap().len() == 9);
    }

    #[test]
    fn steps_report_files_lines_and_checks() {
        let wrote = facts(
            "write_file",
            &json!({"path":"src/a.js","content":"one\ntwo\n"}),
            "abc",
            " · preview OK",
        );
        assert_eq!(wrote, json!({"path":"src/a.js","added":2,"check":"ok"}));
        let edited = facts(
            "edit_file",
            &json!({"path":"a.js","old_text":"x","new_text":"y\nz"}),
            "abc",
            " · preview error",
        );
        assert_eq!(
            edited,
            json!({"path":"a.js","added":2,"removed":1,"check":"error"})
        );
        let plan = facts(
            "plan",
            &json!({"files":["a.js","/workspace/abc/b.css",7]}),
            "abc",
            "",
        );
        assert_eq!(plan, json!({"files":["a.js","b.css"]}));
    }
}
