# Changelog

Each release's section is also its GitHub Release notes.

## v1.5.0

**New**

- **Files on a Slurm cluster.** With **Reach files on the cluster** ticked on a Slurm connection, the model can list, find, read, search and measure each person's own files there, write files, move them, and move them to a trash directory, as that person and nowhere else. The first call in a while starts a small job as the person through slurmrestd with their own token; the job connects out to Chat, carries out the chat's file operations one at a time under the directories the cluster allows (`~` by default; `~,/scratch/$USER` for a scratch space), and exits once idle for an hour or at its time limit. Nothing on the cluster listens, no shared account reads anyone's files, and a job whose account is not the person's ends the session. Writes, moves and trashing wait for **Allow** like any other change, with the path and size on the card; the trash is `~/.chat-trash/<date>` on the cluster, where files keep their paths and can be moved back. The connection takes the partition and QoS for the session job (a small one that starts within seconds), the session length in hours, and the directories the model may reach. The compute nodes need `python3` and must reach Chat's address (`OBLETH_CHAT_ORIGIN`). The Clusters page shows the person's own file session under the cluster, with its job, node and last call, and **End session**, which cancels the job; `GET /api/connections/{id}/status` carries `fileSession` and `DELETE /api/connections/{id}/file-session` ends it. Certification gains six files cases (K06 to K11), two of them a file holding instructions the model must not act on, with their fixtures under `evals/certify/fixtures/cluster-home`. Migration `075` adds `cluster_sessions` and `cluster_calls`. Over the API, the session job uses `POST /api/cluster-sessions/hello`, `GET /api/cluster-sessions/next`, `POST /api/cluster-sessions/calls/{id}` and `POST /api/cluster-sessions/bye` with its own session token.
- **Decisions can allow a change the person asked for.** Before the Allow card for a cluster change that only adds something (a job submission, a new file, a move), Chat puts four yes/no questions to a [decision](https://www.obleth.com/docs/run-chat/decisions): did the person ask for it in their own messages, does it stay within that request, does it keep existing data, and could they undo it. A sure yes to all four (over 70% each) runs the call without the card; the step says "allowed by a decision" and keeps the answers. Anything else, a doubt, an unanswered question or no answer at all, asks as before. Changes that delete or replace something (trash, overwrite, cancel, Kubernetes apply and delete) and tools of other services always ask. The judge reads only the person's own messages and the card, never what the model read from files or pages, and the model is told that text from files on a cluster is data, not instructions. Administrators turn it off with **Let decisions allow changes the person asked for** in Admin › System › Decisions. Migration `076` adds the setting, on.
- **How far each model is trusted with changes.** Under Admin › Models, **Changes through connections** is decided from the model's certification score unless set by hand: a model at 90% or more is **certified**, so a decision may let a change it proposes run unasked; below that, or not certified yet, it is **provisional**, and every change asks; **blocked** offers it only the tools that read, and tells it a change needs a certified model. Chats on `auto` count as provisional. Certification scores come from `evals/certify/publish.py`, which sends each model's result to `PUT /api/admin/model-certification/{model}`; the model editor shows the score, the cases and the date. Migration `077` adds the setting and the score.
- **A better suited model, offered once.** When a chat on a model that is not certified for changes first reaches for a cluster or workspace tool, the reply carries a card: this model is not as well suited to work like this, and the best certified model the person can use did better, with its score. **Switch** uses that model for this chat from the next message on; dismissing keeps the model; **Don't suggest models again** turns the offers off, as does **Suggest a better suited model** under Settings › Appearance. Nothing is forced, and nothing is offered when no certified model is available. Each offer and its outcome is kept in `model_suggestions` (migration `078`), so the offers can be judged.

## v1.4.0

**New**

- **Devices: a board plugged into your computer, reached through your browser.** Under **Compute › Devices**, pick a serial port in the browser's own chooser and name it; it becomes a connection whose tools the model uses in chats. The tools are named by what they do over the port, never by a board: open and close the port, read what it printed (waiting for a marker if asked), send a line or hex bytes, reset or set DTR and RTS, identify the chip, write firmware from the chat's workspace at the offsets the model names, and erase the flash. Each call goes to the browser tab showing the chat, which carries it out over Web Serial and posts the result; the step shows what the tab is doing, or an **Attach** button when the device isn't attached in that tab. Sending, resetting, flashing and erasing wait for your **Allow** like any other change; the flash card lists every image and its offset. Identify, flash and erase support Espressif chips today through esptool-js, with other families as pluggable flashers; any serial device can be read, sent to and reset. The Devices page has a serial monitor per attached device. Ask about hardware before adding one and the reply ends with an **Attach device** card: pick the port, name it, and the task continues with the device's tools. Over the API that is the `request_device` tool and a `device-request` step.
- **Secrets: values the model uses without seeing them.** Under **Settings › Secrets**, save a password or key under a name. The model is told the names; a tool that takes a secret name gets the value where it runs, and the value never enters a chat, a prompt or a gateway request. The first use is a device's **Console sign-in**: set a user name and a secret under Compute › Devices, and the model's **Sign in at the console** call has your browser answer the board's login and password prompts itself. Migration `074` adds `secrets`; over the API, `GET`/`PUT`/`DELETE /api/secrets/{name}`, and `GET /api/secrets/{name}/value` for the owner's own browser. Chrome and Edge only (Web Serial); a device is yours alone and can't be shared. Migration `073` adds `device_calls`, and `OBLETH_CHAT_CONNECTIONS=mcp` turns devices off along with clusters. Over the API, device steps carry `connectionId` and `request`, `GET /api/conversations/{id}/device-calls` lists calls waiting for a browser and `POST /api/conversations/{id}/device-calls/{call}` answers one.

## v1.3.0

**New**

- **Download just the files you pick.** The workspace's Files tab has a tick box beside every file and folder. Tick what you want and choose Download: one file comes as itself, anything more as one zip that keeps each file at its place in the project. A ticked folder brings everything in it, without dependencies and build output. The download button in the bar above still takes the whole project. Over the API, `GET /api/conversations/{id}/workspace/download` takes repeated `path` parameters (up to 200, and 64 MB in all).

**Changes**

- **Memory keeps only what lasts.** Memory kept details of the task in front of it as facts about the person: a file name, a goal for one experiment, a preference stated for one deck. Every memory it would add or change is now put to a [decision](https://www.obleth.com/docs/run-chat/decisions) first, which asks whether it is a lasting fact the person's messages state, worth knowing in chats about other things weeks from now; only a yes over 50% is kept. On a set of 20 proposals, the gateway's verdicts model and gemma4-31b-it each dropped all 10 task details and kept 9 of 10 lasting facts. Administrators can turn it off with **Check memories before they are kept** in Admin › System › Decisions; with decisions off, Memory keeps what it picks, as before. Migration `072` adds the setting, on.

**Fixes**

- **A memory's new topic shows with the memory in it.** A topic was saved before the memory was linked to it, so the Memory page could briefly show a topic with no memories.

**Security**

- **The web interface is reached only at its own address.** Chat sends every request that none of its own routes answers to the interface server. A request path written as `//host/…` or `/\host/…` was resolved to that host instead, so Chat would fetch any address it could reach and return the answer from its own origin, cookies included. The address is now always the interface server's, only page loads (`GET` and `HEAD`) are passed on, the session cookie and other credentials stay in Chat, and the interface server cannot set cookies. Installs behind a proxy that rewrites such paths (Traefik does) were not reachable this way from outside.
- **Large uploads are read after sign-in.** Starting a batch and adding a file to a knowledge collection read their bodies (up to 96 MB and 50 MB) before checking who was asking, so anyone who could reach Chat could fill its memory. Both now check sign-in first, a batch's rows and columns are counted while the spreadsheet is read instead of after, two spreadsheets are read at a time, and an upload has five minutes to arrive.
- **An approval covers one tool call.** With models whose tool calls Chat reads from the reply's text, or that send calls without ids, a later call in the same turn could reuse an earlier call's id and run on its Allow (or be refused on its Deny) without a card. Every call in a turn now has its own id, each question starts unanswered, and an answer is used up when it is read.
- **Shared chats keep the owner's memories private.** What Chat recalled or learned about the owner while answering was part of each reply's activity, which a shared chat showed and a fork copied. Viewers and forks no longer get those entries. A fork also copies only the generated images the shared replies show, not ones from replies the owner regenerated or branched away from.
- **Kubernetes tools take names, not paths.** A namespace, resource name or API version went into the request path as the model wrote it, so one containing `/`, `..`, `?` or `#` addressed something else on the cluster's API, such as a pod's proxy or a whole collection, and the read tools ask for no approval. Each is now checked to be a name of its kind, and no request leaves the cluster address the connection was saved with.
- **An approval card shows what will run.** The card for a Kubernetes or Slurm change was worked out in the browser from the text of the call's arguments, with the text itself folded away and cut at 2 KB. A crafted call could make the two differ: a script line that looked like a field ended the script shown, a manifest after `--- # comment` was applied but not listed, and an extra argument could supply the name a delete appeared to target. The summary now comes from the server, from the same reading of the arguments that carries the call out, and every argument is shown in full and in view, for other services' tools too. A call whose arguments are too long to show whole (384 KB) is refused, and Chat's own tools that change something refuse arguments they do not name.
- **Importing chat history stays within memory.** A small export file could ask for many times its size once parsed, or inflate from a zip, and one upload could take the server down. Each conversation is now held to an allowance as it is read (64 MiB for its bytes and what is kept of them), fields Chat does not use are skipped instead of kept, and an import keeps at most 256 MiB of text (was 768). The file with the conversation that goes over is refused with a message saying so. An upload that stops arriving is dropped after 30 seconds, so it no longer holds the one import that runs at a time.
- **Reading documents takes time and memory that follow their size.** A web page of unclosed headings or math took time that grew with the square of its length (minutes for a megabyte) on the threads that answer requests, and a workbook or deck of empty elements took most of a gigabyte. Pages are now read in one pass, elements are read as they are needed, a document is split at up to 10,000 headings and reads up to 10,000 PDF bookmarks, and all of it runs off the request threads. `pdftotext`, `pdftohtml` and `unzip` run with a 1 GiB memory limit and without Chat's environment, which holds its keys.
- **Batch results are written out as they are read.** Downloading a batch's results held the gateway's output file, every answer and the finished spreadsheet in memory at once, up to 512 MB a file, for as many downloads as were asked for. Answers are now read a line at a time and the spreadsheet is sent a few hundred rows at a time, two downloads run at once, and a download reads at most 192 MB of results.
- **A tool call the model only shows is not run.** For models whose tool calls Chat reads from the reply's text, a call inside a code block, a code span or a quoted line ran like any other, so a reply that quoted a page or file holding one ran it. Those are now left as text. A call's own arguments may still hold code blocks.
- **Revoking an API token stops the reply it started.** A reply started over MCP kept running, tools included, after its token was revoked. It now ends within 15 seconds.
- **Kubernetes `get` shows a Secret without its values:** its keys and the size of each, as `kubectl describe` does. Reading needs no approval and goes to the model, so the values stay on the cluster.
- **A shared connection's address is shown without its path** to people who do not manage it, since the path or query of an MCP server's address often carries its key. The audit log records the address without its query.
- **Drafts end with the session.** What was set up for a chat not yet started (Auto mode, connections switched off, a project, collections) and text typed into Steer stayed in the page after sign-out, for the next person to sign in on it.
- **Knowledge uploads wait for room.** A file's name, type and the person's remaining storage are checked before it is read, and uploads share the memory budget attachments already use, so many large uploads at once wait their turn.
- **Drawings and code blocks cannot hang the page.** Telling whether an `svg` block is a drawing took time that doubled with each comment it opened with, so about 300 characters in any message, a shared chat's included, froze the tab of whoever opened it. It is now one pass. A code block labelled `constructor` or the like is shown as plain code.
- **Batch results:** a cell starting with `-` that a spreadsheet would read as a formula is written as text. List items (`- first`) and negative numbers are unchanged.
- **TLS trust store.** `rustls-webpki` 0.103.16 and `webpki-roots` 1.0.11, which stop trusting two intermediate certificates whose keys are public.

**Deployment**

- **New installs get the current release.** `deploy/compose.yaml` and `deploy/k8s.yaml`, which `setup.sh` applies as they are, still named the 1.0.0 images, so a fresh install ran 1.0.0 whatever was checked out. They now name this version, and CI and the release workflow fail when they do not (`scripts/check-versions.sh`).

## v1.2.2

**Fixes**

- **Images on models the gateway gives vision.** obleth can describe images for a text-only model (its vision boon), but Chat only counted a model's own vision, so attaching an image to one of those models was refused with "Choose a model with confirmed image support". Chat now counts the boon too, and the model picker shows those models as taking images.
- **Thinking written into the reply.** In a long workspace turn, Kimi could start writing its thinking as the reply, with a stray `</think>`, and the steps showed "No thinking yet". Chat sent each earlier step back to the model without the thinking that led to it, and Kimi, like other thinking models that call tools, expects that thinking in the history. Each step now goes back with its thinking.
- **Cancelled rows in batch results.** In a batch's results spreadsheet, a row a cancel stopped before it ran was marked `failed`. It is now `cancelled`.

## v1.2.1

Tagged but not released; v1.2.2 includes it.

**Fixes**

- **One project suggestion per chat.** After someone answered a project suggestion with "Not now", the model could suggest the same project again a turn later. Chat now offers the suggestion only in a chat that hasn't had one.

## v1.2.0

**New**

- **Batches.** Run one prompt over every row of a spreadsheet, in the background. On the new Batches page, upload a CSV or TSV (up to 50,000 rows), click its columns into a prompt such as "Summarize {abstract}", add instructions for every row if you like, pick a model and a deadline (a day, 3 days or a week), and see the first rows' prompts before starting. New to it? **Try the example** loads a short spreadsheet with the prompt already written, and you can download it to see the format. Each row becomes one request that the gateway runs as you, while the model has room to spare, so people chatting never wait behind a batch. The page shows each batch's progress bar, the time left at its pace, and when it is waiting because the model is busy; cancel one at any time. **Download results** gives back the same spreadsheet with an answer, status and error column added, and works while the batch runs, with the rows still to come marked as waiting. Batches are private to each person. Rename a batch and give it tags (only you see them), filter the list by running, done, needs a look or cancelled, by tag, or by words, and delete batches that have stopped, one at a time or several at once; deleting removes the answers and the uploaded spreadsheet for good. Needs an obleth gateway with batches turned on; the page says so otherwise.

## v1.1.0

**New**

- **Memory.** Chat learns lasting things about each person across chats: their role and field, what they work on, the tools they use, their goals and how they like answers. After a reply, a model the administrator chooses reads the person's new messages, as them, and adds, updates or forgets short memories, each linked to the topics it is about. It never keeps health, religion, politics, finances, grades, secrets or anything about other people, and temporary chats are never read.
- **Memories in replies.** A reply gets the memories that fit the message, plus the ones the person pinned. Under the reply, "Remembered 3 things about you" lists them with **Forget**, and anything Chat learned from the exchange shows with **Undo**.
- **Memory page.** A graph of you, the topics your memories are about and each memory, which you can drag and zoom, with a list view. Open a memory to edit it, change its topics or kind, pin it so every chat gets it, or delete it. Open a topic to rename it (a name another topic has merges the two), remove it, or forget everything about it. Add memories yourself, turn off using or learning memories, or delete them all.
- **Dropdowns in the theme.** Every dropdown, in Settings, Administration, the Memory page, sharing and project code, now opens a list drawn in Chat's own colors, light or dark, instead of the browser's plain system list.
- **Administration › System › Memory.** Turn memory on, choose the model that keeps memories, and choose whether it starts on for people who haven't chosen. Memories are matched to messages by meaning when knowledge search is set up, and by their words otherwise.
- **Pause the work.** Ask the model to pause, in a Steer note or a message, and it brings the work to a clean stop and writes where it stands: what is done, where it stopped, what is still running and the next steps. Chat saves that to `PAUSED.md` in the workspace with a checkpoint, shows it at the end of the reply and ends the turn, and Auto mode leaves the chat paused until you write. The next turn starts from `PAUSED.md`. Before, a model asked to pause in a workspace was sent back to check its work and carried on.

**Fixes**

- **A Preview tab that vanished.** On a server without a preview domain, a workspace chat could show a Preview tab for its first minute and then lose it. The tab now shows only once Chat knows previews are set up.
- **Two Chats on one preview domain.** `OBLETH_CHAT_PREVIEW_HOST_SUFFIX` (such as `-dev`) is added to each preview's name, `<id>-dev.<domain>`, so a second Chat can share the first one's wildcard DNS and certificate and answer only its own previews.
- **Tab rows that scrolled.** The workspace pane's tabs and Administration's sections showed a tiny scrollbar beside them. They no longer do; a row too long for a narrow window still scrolls sideways.

## v1.0.9

**New**

- **Project suggestions.** When someone describes work they'll come back to over many chats, such as a thesis, a course they teach or a lab's ongoing analysis, the model can suggest a project. The card shows the project's name, what it's for and the instructions its chats will follow; **Start project** opens New project filled in with them and moves the chat into it. It is offered only in a saved chat that isn't in a project yet.

**Changes**

- **Sign-in log.** When Chat can't refresh the identity provider's discovery document, the log now says why: a timeout, a refused or reset connection, or the status the provider answered.
- **Certification.** Each set has six project-offer cases (106 per set), including ones where no project should be offered.

## v1.0.8

**Fixes**

- **Models that go in circles on one file.** A model that kept rewriting one file without running anything, or kept putting back a version it had replaced (two `Cargo.toml` files in turn, each "fixing" the other), could spend a whole turn that way. Chat now tells it that nothing else changes its files and to move on to the next file. If it loops again in a chat on Auto, Chat asks the gateway for a stronger model for the rest of the turn, and the steps show "Chat moved this turn to a stronger model".

## v1.0.7

**Changes**

- **What a card offers, over MCP.** `get_chat` now reports what each waiting card offers: an app's name, description and steps, a research suggestion's goal and metric, and the reason a workspace was asked for.

**Fixes**

- **Deleted chats' workspace servers.** Deleting a chat left the server its workspace started running, and its project folder on disk. The next new workspace was given the same port and could not start its preview. Chat now stops those servers and removes those folders when a workspace starts and every few minutes.
- **Certification.** Each chat is checked every 20 seconds, so 28 cases at once stay under a token's 120 calls a minute, and the judge sees what an app, research or workspace card offers.
- **Certification runs at scale.** An account's workspace chats share one pod and its browser chats one browser, so the runner limits how many such cases run at once across every run on the machine, and cases marked `heavy` have their own few slots. Each record says when it ran, so a failure can be matched against a pod restart. The runner's own stop at Allow no longer counts as a failed reply, a judge prompt stays within Chat's 12,000-character message limit, a multi-turn case stops when a turn runs out of time, and a chat's start may wait for its workspace.

## v1.0.6

**Changes**

- **Research suggestions.** When someone asks for something tuned or optimized over many measured tries, the model now suggests a research project before it starts, even when data or a baseline has to be made first. Before, models often built the baseline themselves and never offered one.
- **Certification.** A change that stops at Allow is no longer mistaken for one that ran without approval when the runner stops the chat. A `calls` rule can name alternatives (`run_command|start_server`), and nine set-2 cases were reworded so they don't depend on files the workspace doesn't have.

## v1.0.5

**New**

- **Delete chats over MCP.** `delete_chat` deletes one of the person's chats, as Delete does in the browser, so a client that starts many chats can stay under the 1,000-chat limit. It needs a token that may start chats, and it is recorded in the audit log.

**Changes**

- **Certification.** The runner deletes each test and judge chat once it is graded (`--keep` keeps them), checks on chats less often so a dozen cases at once stay under a token's 120 calls a minute, and waits out that limit instead of failing. `cleanup.py` removes chats an interrupted run left, `rescore.py` merges and regrades runs, and `cases-people.yaml` adds a second set of 100 everyday requests. Chat's own search of attached collections no longer counts as the model calling `search_knowledge`.

## v1.0.4

**New**

- **Past Slurm jobs.** The Jobs tool can list jobs that ended in the last 30 days, with their state and exit code, and the Clusters page shows a cluster's jobs from the last 7 days under **Recent jobs**.

**Changes**

- **12 replies at once.** A person may now run 12 replies at once by default, up from 3. A site still at the old default of 3 moves to 12; one that chose another number keeps it.
- **Knowledge search.** Models search a person's collections and saved items only when the answer may be in that material, not for general questions.
- **Certification.** The cases and runner in `evals/certify` were corrected from the first gold runs. The judge no longer fails a reply because it remembers a page differently from what the assistant read.

## v1.0.3

**New**

- **Collections from a project.** A project's page and its collections picker have **New collection**, which makes a collection named after the project and turns it on there. **Add** next to one of your own collections uploads documents straight into it.
- **Administrators' connections.** An administrator's own connections may reach internal addresses, as shared ones already may. A connection refused for a private address now says so instead of "Could not connect".

**Fixes**

- A long title no longer widens a dialog past its frame.
- Delete buttons in confirmations show in red; the default button colors no longer override them.

## v1.0.2

**New**

- **Feedback button.** People send a problem, an idea or anything else from the sidebar. Administrators read it under Administration › Feedback › Sent feedback, filter it, mark it done, and choose whether the button shows highlighted, plainly or not at all. A sender is named only when they say they may be contacted.
- **MCP.** `start_chat` can start the chat in a project, turn on its browser and choose which connections it may use; `get_chat` lists the latest reply's steps.
- **Certification.** `evals/certify` holds 100 cases over Chat's workflows and a runner that scores models on them through the MCP endpoint.

**Fixes**

- A question at the end of a reply now shows as a question card instead of being written into the text.
- The model offers an app only for something that answers requests, such as an assistant over documents or an API. A site for sharing files, a portal or a dashboard is built in a workspace instead.

## v1.0.1

**Fixes**

- A workspace or browser that was starting when Chat restarted no longer shows Starting for good. Chat now settles a start that is overdue: running if its pod came up, otherwise an error with Start again.
- The workspace page keeps checking a starting workspace when one check fails, as while Chat restarts, instead of waiting on Starting.

## v1.0.0

The first release of obleth-chat: a self-hosted chat app in front of the [obleth gateway](https://github.com/thediymaker/obleth-gateway), where each person signs in with their own account and every request reaches the gateway as that person.

**What's in it**

- **Chat.** A model or Auto, thinking effort, files and images, versions you can step through when you edit or regenerate, branches, temporary chats, read-only and live sharing, and replies that keep running on the server when the tab closes. Replies draw Markdown, math, Mermaid diagrams, molecules, SVG and CSV.
- **Knowledge.** Collections of PDFs, Word files, HTML, Markdown and code, searched with citations that open the source at the passage.
- **Projects.** Shared instructions, documents, notes and chats, with the project's code in one git history, exported as a zip or pushed to GitHub.
- **Workspaces and the browser.** A private Linux pod per person where the model writes, runs and fixes an app, shown in a live preview with files, a terminal and checkpoints; and a Chrome the model drives with the person watching.
- **Connections.** MCP servers, Slurm clusters and Kubernetes clusters as tools, acting as the person's own account. Anything that changes state waits for Allow or Deny.
- **Research and agents.** Research mode runs an experiment tree in a project; worker agents take tasks in the background.
- **Apps.** An administrator turns a project into an app: steps over typed inputs (Agent, Search knowledge, Code, Decide, If, For each, HTTP request, Response), laid out on a canvas, tried as a draft, then published as an API called with a key and as a chat page at `/a/<code>/<name>`. A chat can suggest an app with its first steps.
- **Administration.** Models and who sees them, generation defaults, daily allowances, accounts and groups, usage and insights, activity, feedback, and a site banner.
- **Sign-in.** Local accounts, OIDC, or both. With OIDC, Chat can forward each person's own access token to the gateway. Signing in returns to the page it started from.

**Running it**

- Two servers: the Rust server (`chat` image), which is the front door and answers every API call, and the interface server (`chat-ui` image, Next.js), which the Rust server sends every page request to through `OBLETH_CHAT_UI_URL`. Workspaces and the browser have their own images (`workspace`, `browser`).
- `./setup.sh docker` runs both with PostgreSQL through Compose (`deploy/compose.yaml`); `./setup.sh kubernetes` runs them with workspaces (`deploy/k8s.yaml`).
- Branding is `site.config.js` and `assets/` in the branding folder (`OBLETH_CHAT_BRANDING_DIR`, `/app/branding` in the image), which installs replace with their own.
