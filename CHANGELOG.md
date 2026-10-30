# Changelog

Each release's section is also its GitHub Release notes.

## v0.14.1

Models are told when to suggest research.

**Fixed**

- **Research suggestions.** In a chat that may suggest research, the system prompt again includes the guidance on when to call `suggest_research`: tuning or optimizing over many measured tries, a parameter sweep, or a run left going overnight. Since v0.12.0 it was added after the system prompt had been put together, so models saw only the tool's description and could plan the suggestion without making it.

**Upgrading**

- No migrations or configuration changes.

## v0.14.0

Administrators can show a one-line banner, such as "In development" or an announcement, in the top bar of every page and, if they choose, above the sign-in page.

**New**

- **Banner.** Settings › Administration › System has a Banner card: one line of text (up to 200 characters), an optional link, a Notice or Warning style, whether it also shows on the sign-in page and whether people may close it. Empty text turns it off.
  - A closed banner stays closed in that browser until its text or link changes.
  - Links go only to `http`, `https` or `mailto` addresses. On a phone the link is hidden to keep the top bar on one line.
  - `GET` and `PATCH /api/admin/banner` read and change it, and each change is recorded in Activity under settings.
  - `GET /api/session` gains `banner`: `null` when there is none, otherwise its text, link, style and whether it can be closed. Signed out, it is `null` unless the banner is set to show on the sign-in page.

**Upgrading**

- Migration 056 adds `banner`, with no text, so nothing shows until an administrator sets one. It runs when Chat starts.

## v0.13.1

A turn with tools has no step or time limit in any chat. Auto mode now only decides whether tools run without asking.

**Changed**

- **No step or time limit.** A turn with tools no longer pauses after 100 steps or 3 hours, with or without Auto mode. It runs until the model finishes or you choose **Stop**, and still pauses when the model sends nothing for 10 minutes. The model sees the latest 150 steps in full, as before, so a long turn's requests stay bounded.
- **Auto mode is about asking.** In Auto mode every tool runs without asking, the turn keeps going after the sign-in that started it ends, and a request the model stalls on is retried twice before the turn pauses.

**Upgrading**

- No migrations or configuration changes.

## v0.13.0

A reply's details show how fast it was written and, when the gateway measures it, the energy and CO₂ it used. Activity reads as one sentence per event and can be filtered. Every chat knows today's date. Workspace cleanup can no longer hang workspace and browser starts, and an asleep workspace wakes from its preview.

**New**

- **Speed and energy in a reply's details.**
  - **Speed** is output tokens over the time spent writing them. Chat times each step from its first generated token (text, thinking or a tool call) to its last, and counts a step only once its usage arrives, so a dropped attempt doesn't slow the figure. It is stored in the reply's usage as `generation_ms`.
  - **First token** is how long the first step took to write its first token, stored as `first_token_ms`.
  - **Energy** and **CO₂** come from the gateway's `usage.energy_wh` and `usage.co2_g`, summed over a reply's steps like tokens. A gateway that doesn't send them shows neither.
  - Older replies, and replies too short to time (under 200 ms or one token), show no speed.
- **Activity filters.** A kind menu (accounts, sign-in, groups, settings, connections, MCP, cleanup) and a search box sit above the list, and an empty filtered list offers **Clear filters**. The search matches the names of the actor, the target and a group's member, the action, the target id and a connection's name, ignoring case.
  - `GET /api/admin/audit` takes `kind` and `q`. An unknown kind, or a search over 128 characters, is a 400.
  - Each event gains `kind`, `targetName` (the person, group, model, connection or token by name) and `memberName` (the person in a group-membership or ended-sign-in event).
- **Wake from the preview.** On a workspace's Preview tab, reload wakes an asleep workspace or restarts a stopped app, and the preview opens once the app serves. The asleep note has a **Wake it up** button. Neither needs a message to the assistant.

**Changed**

- **Activity reads as sentences.** One sentence per event with an icon for its kind, under day headings, with the time on the right and the full date on hover. Details are label and value rows instead of JSON.
- **Cleanup shows only when it did something.** Automatic cleanup records an Activity event only when it removed something. Every run still updates the retention policy's last run and result, which the System tab shows.
- **Every chat knows today's date.** Every turn's system prompt starts with today's date, not only a workspace turn, so web and browser searches about anything recent name the right year. The browser's instructions ask for the year or date in such searches.
- **Compute.** The Workspaces and Clusters tabs share the heading "Compute". A workspace is described as a project folder in your own Linux container, on the page, in the workspace offer and in the model's `turn_on_workspace` tool.

**Fixed**

- **Workspace and browser starts no longer hang.** Workspace cleanup marked an idle workspace stopped inside the transaction that holds its lock, then waited on another connection updating the same row, so the transaction never ended. Every later workspace or browser start for that person waited behind it, with no pod and no log line. Cleanup's writes now commit at once, and every database connection sets `idle_in_transaction_session_timeout` to 10 minutes, so a transaction left open is ended and its locks released.
- **Feedback** cards have inner padding, and every management table right-aligns its last column's header as it does the cells.

**Upgrading**

- No migrations or configuration changes.

## v0.12.0

Clusters show what each Slurm partition can run now instead of every node, and a cluster's nodes are listed at most once every 30 seconds for everyone.

**Changed**

- **Partitions, not every node.** The Clusters page shows each partition's idle nodes, idle CPUs and, on a cluster with GPUs, idle GPUs, each out of the partition's total, with its state and time limit. The tiles show CPUs, GPUs and memory in use, how many nodes are idle or unavailable, and the queue. The node table is gone.
  - Counts follow `sinfo`: idle CPUs and GPUs are those free on nodes that can start jobs, and an idle node has nothing running.
  - Nodes that are down, drained, failing, not responding or in maintenance are unavailable and never count as idle.
  - GPUs come from each node's `gres` and `gres_used`.
- **The Cluster tool gives the model a summary.** One line per partition, plus one for the cluster and the queue, in place of one line per node. A new optional `partition` argument also lists that partition's nodes, for a partition the person's account sees.
- **A cluster's nodes are listed once for everyone.** Each server lists a cluster's nodes and reads the scheduler's statistics at most once every 30 seconds, and everyone who looks in that time shares the result. Partitions and the person's own jobs are still asked with the person's own token every time. Each person sees only the partitions Slurm shows their account, totals count only the nodes in those partitions, and the token is checked even when the nodes come from someone else's look. A page view now makes two slurmrestd calls instead of four.

**Upgrading**

- No migrations or configuration changes.

## v0.11.0

Any file can be attached to a chat: a zip of a web page and its images, a dataset, a tarball of code, a long log. The model is told what each file holds, and the chat's workspace gets a copy.

**New**

- **Attach any file.** The paperclip, drag and paste take files of any kind. Text up to 256 KiB, PNG, JPEG and WebP images up to 5 MiB, and PDF and Office documents work as before. Everything else is kept as a file: archives, binaries, data such as CSV, Parquet, HDF5 and NumPy, SVG, images too large to view, and text too long to read whole.
  - The model sees each file's name, size and kind. For a zip or a plain tar it also sees the list of entries, read without unpacking, up to 200 entries with a count of the rest. For a text file it sees the first 4 KB, marked as a preview.
  - A file's chip shows an archive or file icon, its name, its size and, for an archive, how many files it holds. Dropping a folder says to zip it first.
- **Files land in the workspace.** Before each message in a chat with a workspace, every file sent in the conversation, of any kind, is copied to `uploads/<name>` in the project folder. A workspace that is asleep gets them when its first tool wakes it. Archives stay packed, and the model unpacks what it needs. Two different files with the same name become `data.csv` and `data-2.csv`. `uploads/` stays out of the project's git repository and of its checkpoints. The workspace image adds `unzip`, `xz-utils` and `bzip2`.
- **Limits are set by the server.**
  - `OBLETH_CHAT_MAX_FILE_MB` sets the largest file or document, 50 MB by default (was 20 MiB for documents).
  - `OBLETH_CHAT_FILE_QUOTA_MB` sets one person's files in all: 1024 MB by default with object storage, 200 MB without (was 50 MiB).
  - `OBLETH_CHAT_MAX_FILES` sets how many files one person may keep, 500 by default (was 100).
  - `GET /api/attachments` returns the limits in effect, and the web client checks files against them.

**Changed**

- **A file is always downloaded, never shown.** A file kept as `file` is served as `application/octet-stream` with `Content-Disposition: attachment` and `X-Content-Type-Options: nosniff`, whatever its name or stated type, from your chats and from shared chats alike. An uploaded HTML page or SVG can't run on Chat's site. Images and PDFs are served as before.
- **Uploads are bounded by size, not count.** Each server holds at most 256 MiB of upload bodies and workspace copies at once, weighed by size, in place of eight uploads at a time. An upload must declare its size: one without `Content-Length` gets 411, and one over the limit gets 413 before any of it is read.
- **Uploads can be sent as raw bytes.** `POST /api/attachments?name=<name>&type=<type>` with `Content-Type: application/octet-stream` avoids base64, and the web client uses it. The JSON form still works.
- **Downloads from object storage are streamed** instead of held in memory, and requests to object storage get a timeout that grows with the size of the file.
- **Exports leave out files too large to fit.** The message gets a line naming the file, the archive lists it in `leftOut`, and the History page says so. Text, images and documents still have to fit, as before.

**Upgrading**

- Migration 055 adds the `file` attachment kind and drops the fixed upper limit on `attachments.size`.
- Behind a reverse proxy, allow request bodies a little over `OBLETH_CHAT_MAX_FILE_MB` for `POST /api/attachments` (about 51 MB at the default), or 4/3 of it plus 1 MB for clients that still send JSON.
- `OBLETH_CHAT_MAX_FILE_MB` can be 1 to 2000, and at most 1000 without object storage, where files are kept in Postgres.

## v0.10.0

Replies draw math, code, diagrams, charts, molecules and tables. A message can carry PDF, Word, PowerPoint and Excel files. Replies can be rated, chats can be temporary, and chat history comes over from Open WebUI, ChatGPT and Claude.

**New**

- **Replies draw more than Markdown.**
  - Math with KaTeX, including mhchem's `\ce{}` and `\pu{}`: `$…$` and `\(…\)` inline; `$$…$$`, `\[…\]` and ```` ```math ```` blocks for display. Prices such as "$5 and $10" stay text.
  - Highlighted code, with extra grammars loaded by name: Julia, Fortran, MATLAB, LaTeX, Dockerfile, Stata, SAS, Verilog and more.
  - Mermaid diagrams and charts in ```` ```mermaid ```` blocks.
  - Molecules from SMILES in ```` ```smiles ```` blocks.
  - SVG drawings in ```` ```svg ```` blocks, shown as images.
  - Tables from ```` ```csv ```` and ```` ```tsv ```` blocks.
  - A drawn block appears once its fence closes and keeps a toolbar to switch to its code, download, copy and save. The libraries ship with Chat and load the first time a reply needs them, and the content security policy is unchanged. Every chat's instructions gain one line telling the model what Chat draws.
- **Documents in a message.** PDF, Word, PowerPoint and Excel files (`.pdf`, `.docx`, `.pptx`, `.xlsx`, up to 20 MiB) attach like other files. Their text is read once, on upload, with a marker where each page, slide or sheet starts.
  - A document whose text fits in 75 KB is sent whole. With Knowledge set up, a longer one is indexed for that chat and searched, with citations; without Knowledge it is refused with a message that names the limit.
  - Scanned PDFs and the older `.doc`, `.ppt` and `.xls` formats are refused with a message. Knowledge collections now take PowerPoint and Excel files too.
- **Rate a reply.** Thumbs up or down on finished replies in your own chats; thumbs down offers reasons and a comment. Administrators see ratings per model, the reasons and recent comments over 7, 30 or 90 days in **Settings › Administration › Feedback**, with a CSV download. They never see the chat, the reply or who rated it.
- **Temporary chats.** A toggle beside the model picker on a new chat. A temporary chat stays out of your chats and search. It can't be shared, exported, branched, moved into a project or saved to the Library. It is deleted when you leave it, and by the server after 23 quiet hours.
- **Import from other apps.** **All chats › Import** reads Open WebUI's chat export, and ChatGPT's and Claude's data exports (the zip or its `conversations.json`), up to 200 MB and 5,000 conversations.
  - Each chat keeps the path its app was showing, and edits and regenerated replies become versions. Each reply keeps the name of the model that wrote it.
  - Images and files become a placeholder line.
  - Importing the same file again adds only what's new.

**Changed**

- **Scrolling up while a reply is written stays put.** The page used to snap back to the end whenever you were within 160 px of it. Now it stops following as soon as you scroll up, by wheel, touch, keyboard or the scrollbar, and follows again once you scroll back to the end, send a message or open a chat.
- **Large requests are bounded per server.** An upload's body is read only after sign-in, eight at a time, up to about 28 MB for a document. History imports run one at a time, from reading the upload to saving it.

**Fixed**

- **A live shared chat scrolled to its end on every refresh.** It measured an element that doesn't scroll, so it always counted as being at the end. It now follows the same rule as your own chats.
- **A Slurm job whose output file name Chat can't follow said its output would come back.** On a cluster set to bring output back, a pattern such as `%4j` ran the job without the upload step but still pointed the model at Job output. The job now runs as asked and the result says its output won't come back.

**Upgrading**

- Migrations 051 to 054:
  - 051 adds `message_feedback`.
  - 052 adds `conversations.temporary` and `left_at`.
  - 053 adds `document` attachments: `extracted_text`, `parts` and `searched`; `knowledge_collections.attachment_id` and `conversation_id`; and makes `knowledge_documents.object_key` nullable.
  - 054 adds `conversations.import_key`.
- Behind a reverse proxy, allow request bodies of about 28 MB for `POST /api/attachments` and 200 MB for `POST /api/conversations/import`, with a timeout long enough to save a large import.
- Documents are read with `pdftotext` and `unzip`, which the published image has. Each one is written to `/tmp` while it is read, so `/tmp` must hold several 20 MiB files at once. A 16 MiB in-memory `/tmp`, as the example deployments had, refuses larger documents and Knowledge files. `deploy/k8s.yaml` now gives it 1 GiB on disk and `deploy/compose.connected.yaml` 512 MiB.

## v0.9.4

Slurm clusters can bring a job's output back to Chat, Job details reads jobs the scheduler has forgotten, and the chat suggests research for long tuning work.

**New**

- **Bring job output back.** An option on each Slurm cluster, offered when Chat has file storage (`OBLETH_CHAT_S3_*`) and off until an administrator turns it on. Each job Chat submits on such a cluster gets a link, valid for 7 days, that can upload one object to Chat's storage. The job runs the person's script unchanged; as it ends, including when it is cancelled or reaches its time limit, it uploads the last 1 MiB of its output file with `curl` and keeps the script's exit code. The output file follows the job's `--output` pattern (`%j`, `%x`, `%u`, `%A`, `%a`, `%N`), or Slurm's default `slurm-%j.out`. The new read-only **Job output** tool shows the end of what the job printed, and says when the job hasn't ended yet or ended without its output arriving. Output is kept for 30 days, and goes sooner with its cluster or person. The cluster's compute nodes must reach the storage and have `curl`.

**Changed**

- **The chat suggests research for tuning work.** When research is offered, the chat is asked to suggest a research project before tuning something, sweeping parameters or leaving a run going overnight, instead of doing that work in a plain chat.
- **The Steer box's example fits the reply.** The placeholder follows what the reply is doing, such as "ask for 4 hours, not 1" while it works on a Slurm cluster or "add tests before you finish" in a workspace.

**Fixed**

- **Job details lost finished jobs.** The scheduler forgets a job a few minutes after it ends, so Job details then failed for it. It now reads such a job from Slurm accounting: its state, exit code, run time and output file.

**Upgrading**

- Migration 050 adds `slurm_job_outputs`.

## v0.9.3

Slurm clusters read only the person's own jobs, Chat caps what it reads from any cluster, and research runs that end as Chat checks on them are no longer marked lost.

**Fixed**

- **A large Slurm cluster ran Chat out of memory.** A cluster's status, and the `jobs` and status tools, read slurmrestd's list of every job on the cluster and kept only the person's; on a large cluster that list is hundreds of megabytes. The person's running and pending jobs now come from Slurm accounting, filtered by their user name, and the queue's size from the scheduler's statistics (`/diag`). The `jobs` tool no longer lists other users' jobs.
- **Every Slurm and Kubernetes answer is capped at 32 MiB** as it is read. A larger one is refused with a message that names the limit, instead of being held in memory.
- **A research run that ended just as Chat checked on it was marked lost.** Chat looked for the run's exit file, then for its process; a run that finished between the two was reported lost and its result dropped. Chat now reads the exit file again once the process is gone.

## v0.9.2

Slurm clusters work with slurmrestd v0.0.43 to v0.0.45, and a cluster can set its REST API version.

**Fixed**

- **Slurm clusters on newer slurmrestd.** Chat looked only for REST API v0.0.40 to v0.0.42, so a slurmrestd that has dropped those answered every call with "does not offer the Slurm REST API v0.0.40 or newer". Chat now looks for v0.0.45 down to v0.0.40 and uses the newest one the server answers.

**New**

- **REST API version on a Slurm cluster.** Optional, such as `v0.0.46`. When set, Chat uses only that version, so a newer slurmrestd works without a Chat release; empty keeps finding the newest. A version the server doesn't offer is refused when the cluster or a person's token is saved.

## v0.9.1

Model load tints the model's own icon instead of adding a dot.

**Changed**

- **Model load tints the model's icon** instead of adding a dot: amber while the model is busy, red while it is full, faint while it has no servers. Load reads "37 running on 32 slots", which stays true while servers being removed finish their requests and the slots already count only the rest.

## v0.9.0

Research reads papers by section, Knowledge cites the section a passage comes from, long pastes become attachments, and Chat shows how busy each model is.

**New**

- **How busy each model is.** With a gateway that reports load (obleth's `GET /v1/load`), the model list marks a model that is busy (amber), full (red) or has no servers running (hollow); a free model carries no mark. The model button shows the chosen model's mark. A reply that hasn't started after 3 seconds says what holds it: the model is full, with how many requests are running and waiting, or the person has reached their own in-flight limit. It offers to switch to a favorite or default model that isn't full, or to Auto; switching stops the waiting reply and sends it again. When the gateway gives up waiting for a slot, the reply says the model stayed full and offers the same switch. Gateways that don't report load show none of this.
- **Research reads papers by section.** `read_paper` takes a link to a paper, or any web page with headings, and what the chat is looking for in it. arXiv abstract and PDF links read arXiv's HTML version. The page is fetched in the workspace, once, and kept there. Chat splits it at its headings, keeping math as TeX. One pick-one decision over the outline ranks the sections, and the likeliest three come back in full, with the outline and each pick's probability. A document with more than 255 sections is narrowed by its top-level sections first. Asked for `sections` by id, number or heading, it returns those. The research rules ask the chat to take a paper's settings from the section that gives them and cite it. Administrators turn the picking off with **Pick sections of papers**; the chat then gets the outline and names the sections itself.
- **Knowledge keeps the section each passage sits under.** PDFs are split at their bookmarks, Word files at their heading styles, Markdown at its `#` headings and HTML at `h1`–`h6`, and a passage never spans two sections. Citations, the sources under a reply and Knowledge search show the section ("2 Method › 2.1 Schedule") with the file and page, and the chat sees it with each passage. Keyword search reads a section's headings as part of its passages, so a word only the heading has finds them. Passages are embedded as before. Files without headings are read as before, and files indexed before this have no sections until they are uploaded again.

**Changed**

- **Long pastes become attachments.** Pasting more than 4,000 characters into the message box, or enough to pass its 12,000-character limit, attaches the text as a file named "Pasted text" instead of cutting it off where the box ends. Click the attachment's name to read it, or remove it. Text over what a conversation can hold (100 KiB) is refused with a message. The box shows how many characters it holds once it is near the limit.

**Fixed**

- A Word or HTML file in which an `&` is followed closely by Chinese, Japanese or other multi-byte text stopped Knowledge indexing on the replica that read it, and on each replica that retried it, until Chat restarted. Such files are read correctly now, and each file is read in its own task, so a file that fails unexpectedly is marked failed on its own.

**Upgrading**

- Migration 048 adds `decisions.pick_sections` (on).
- Migration 049 adds `knowledge_passages.section` and rebuilds the keyword index of `knowledge_passages` to include it. The table is rewritten once.

## v0.8.0

Research checks where each new experiment goes in the tree, and whether a reply's statements match its runs.

**New**

- **Research checks where each new experiment goes.** `experiment_create` takes `builds_on`: what the parent established that the new idea keeps. It is required unless the parent is the baseline, and an experiment that can't name it is pointed to its parent's parent instead. Chat then asks whether the idea builds on its parent; under the baseline, once another node beats it, whether the idea is another option of the round already there rather than a new decision, which belongs under the best result. A confident no refuses the experiment once and says where it belongs; the chat can confirm it with `confirm: true`. The Research tab shows each node's `builds_on` and the check. Administrators turn it off with **Check where new experiments go**.
- **Research replies are checked against their runs.** When a research turn ends, values a reply gives a run (`run 4 reached loss 2.31`, `run 4: 2.31`, or a table with a run column) are compared with what the run printed, at the digits written, and runs that don't exist are named. Plans, targets, differences and running runs are left alone. Comparisons and causes the code can't check go to Decisions in one request. What doesn't match shows as a step under the reply, and the next research turn is told to correct it. Administrators turn the Decisions part off with **Check research reports**; the value checks need no model and always run.

**Changed**

- A Decisions answer that doesn't say how sure it is counts as sure in proportion to how far it is from a coin flip, instead of not at all. The new checks act only on a no that is at least 50% sure.

**Upgrading**

- Migration 047 adds `decisions.check_tree` and `decisions.check_reports` (both on), `experiments.builds_on` and `experiments.placement`, and `research.report_check`.

## v0.7.0

Administrators choose where Chat asks its short yes/no questions, and research checks whether each run tested its idea.

**New**

- **Decisions: where Chat sends short yes/no and pick-one questions.** Administrators choose in **Settings › Administration › System › Decisions**: the gateway (the default), as the person each question is for, so it counts toward their usage; another endpoint with its own key, so usage counts against that key; or off. Questions use the verdicts format (`POST /v1/verdicts`), so any service that speaks it can answer. An address with no path, or one ending in `/v1`, gets `/v1/verdicts` added. The key is sealed with `OBLETH_CHAT_SECRET_KEY_FILE`, never shown or logged, and redirects aren't followed, so it goes only where the administrator pointed it. **Model** left blank lets the endpoint choose; the model must return token probabilities (logprobs). **Check** asks a sample question with the form's values before they are saved. When decisions are off or fail, work goes on without them.
- **Research checks whether each run tested its idea.** When the chat reads a finished run on a changed node, Chat asks whether the run really tested its idea, from the node's description, its change from the parent and the end of its output. When the answer leans no, `experiment_wait` tells the chat to read the output before trusting the number and to put any fix in a child; the node stays frozen either way. The project's Research tab shows the check on each run, or why it was skipped. Administrators turn it off with **Check research runs**.

**Upgrading**

- Migration 046 adds the `decisions` settings (the gateway, with run checks on) and `research_runs.decision`. With a gateway that has no `/v1/verdicts`, run checks are recorded as skipped; choose **Off** to stop asking.

## v0.6.0

Chat can send each person's own identity provider token to the gateway, and can run with single sign-on only.

**New**

- **Forward people's own tokens to the gateway.** With `OBLETH_CHAT_GATEWAY_AUTH=forward`, a gateway call carries the access token the OIDC provider issued to the signed-in person instead of a token Chat signs, so the gateway sees the person and Chat holds no key that can act as anyone. Chat keeps each sign-in's tokens encrypted, refreshes the access token shortly before it expires under a lease (so two replicas never present the same refresh token, and no database connection waits on the provider), in a task a closed tab can't interrupt, and sends it only to the gateway. Each sign-in uses its own grant, and API-token chats the grant of the sign-in the token was made in; Auto mode and Knowledge indexing use the person's most recently refreshed sign-in. When the provider refuses a refresh with `invalid_grant`, Chat ends that sign-in and the API tokens made in it, and work with no sign-in left stops with "Sign in again to continue": Auto mode stops nudging, and Knowledge marks the file failed. `invalid_client` and other client errors end nothing. A failed sign-in revokes the grant its code exchange made. An account can be linked to only one identity at the provider. Someone linked to the provider never gets Chat's signed token; local password accounts still do while a signing key is set. Forward mode needs OIDC with `offline_access` and `OBLETH_CHAT_SECRET_KEY_FILE`; the gateway signing key becomes optional. The gateway must trust the provider's issuer, keys and audience. See the README.
- **Signing out revokes the provider's refresh token.** In forward mode, every way a sign-in ends (signing out, revoking a session, signing out other devices, a password change or reset, an administrator disabling the account or ending its sessions, a session expiring) revokes its refresh token at the provider's `revocation_endpoint` when discovery lists one, in the background so signing out never waits.
- **Sessions follow the provider in forward mode.** A sign-in that keeps a grant lasts `OBLETH_CHAT_SESSION_HOURS` (now up to 8760) rather than ending with the ID token. Once its access token expires, its next request of any kind refreshes it, and a refused grant ends the session then, so ending the sign-in at the provider ends it in Chat within the access token's lifetime. A provider that can't be reached is trusted for 15 minutes past expiry and asked again for a sign-in at most every 30 seconds, an access token that has not expired keeps serving meanwhile, and signing out never needs the provider. A refresh token sent with no answer back is presented again only within 45 seconds, since a provider that rotated it may take a later reuse as theft; after that, or when a replica stopped in the middle of a refresh, Chat ends the sign-in itself. Shutdown starts no refresh and sends no token, and waits for refreshes under way to save what the provider answered. Password sessions stay capped at 168 hours. Installs usually set something like 2160 hours.
- **`OBLETH_CHAT_PASSWORD_SIGN_IN=false` turns passwords off.** Password sign-in, password changes, administrator resets, invitations and reset links all refuse, the sign-in page shows only the single sign-on button, and administrators create accounts without a password and link them to single sign-on. It needs OIDC, and Chat refuses to start with a bootstrap password rather than leave one waiting in the environment. Pending forced password changes are cleared, since no one could make them. Sessions that began with a password, and the API tokens made in them, end when Chat starts and are refused on every request. Link existing administrators before turning passwords off.
- **`OBLETH_CHAT_OIDC_PROVIDER_USERNAMES=true` names accounts after the provider's username.** An account made at sign-in takes the provider's `preferred_username` when it is a valid username, doesn't start with `oidc_` and no other account has it; otherwise it is named `oidc_` plus an id as before. Names taken this way follow the provider: on each sign-in they change to the provider's current username, or back to a generated name when another account holds it. Accounts still on a generated name take the provider's once it is free, including accounts made before the setting was turned on. Names an administrator or bootstrap chose are never changed. Off by default, since OIDC doesn't promise that this claim is unique or stable: turn it on only for providers that assign usernames themselves and never reuse them.
- **`OBLETH_CHAT_OIDC_SCOPES`** sets the scopes Chat asks for (default `openid profile`; `openid` is always included).
- **`OBLETH_CHAT_SECRET_KEY_FILE`**: a 32-byte key (raw, hex or base64; `openssl rand -hex 32`) for what Chat stores encrypted. When set, saved connection tokens are sealed with it, and while the gateway signing key is also set, tokens sealed with the key derived from it move to the secret key at startup and every five minutes, even with several replicas running together. Until they move they still open with the signing key. Adding the secret key is one-way: without it, moved tokens can't be read.

**Fixed**

- **Linking an identity and that identity's first sign-in no longer race.** Administrators linking an account take the same lock as a first sign-in, so a sign-in finishing at the same moment no longer fails.
- **ID tokens must use an asymmetric algorithm.** Chat accepts only the RSA, RSA-PSS, ECDSA and EdDSA algorithms the provider advertises. A provider advertising HS256 let an ID token signed with the client secret through; HS* and `none` are now always refused. Keys still come only from the discovered `jwks_uri`, which may be on another host than the issuer.

**Upgrading**

- Migration `045` adds `users.username_from_provider`.
- Migration `044` runs when Chat starts: it adds `oidc_grants` and `oidc_revocations`, and records whether each session and API token began with single sign-on. Existing ones count as single sign-on only for accounts without a password.
- `deploy/compose.connected.yaml` still runs sign mode with passwords on; forward mode and passwords off need a deployment that sets their variables.
- `/api/session` and `/api/capabilities` report `passwordSignIn`; `features.localAuth` follows it.

## v0.5.2

Security fixes for workspace previews, uploads, personal tokens, sign-in limits and shared chats, and a simpler sign-in page.

**New**

- **`/metrics` can require a token.** Set `OBLETH_CHAT_METRICS_TOKEN` and `/metrics` answers only requests with `Authorization: Bearer <token>`; others get 401. Unset, it stays open as before.

**Changed**

- **The sign-in page is one quiet column**: the brand and theme switch at the top, a plain heading, the form, and the footer links, in place of the two panels with a large slogan. When single sign-on is configured, its button leads and the password form comes second. `login` in `site.config.js` now takes `title` (blank reads "Sign in to" and the site name), `description`, `ssoLabel`, `ssoHint`, `help` and a new `helpLink` (`{ label, url }`) at the end of the help line. The invitation, password change, session check and error screens use the same layout, and an expired session shows a notice instead of an error.
- **Making a personal API token needs a sign-in from the last 15 minutes** (was 12 hours, as long as a session lasts), so a borrowed or stolen session cannot turn into a 30-day token. Otherwise the web app offers to sign out and back in.

**Fixed**

- **Previews only reach the workspace's own pod.** Before proxying, Chat checks with the API server (at most every few seconds per pod) that the workspace pod is still running at the address on record. A pod that was evicted, deleted or lost with its node is marked stopped instead of its old address being used, which another pod may have been given. The reconciler does the same every minute.
- **Preview access ends with the sign-in.** Preview tickets and the preview cookie carry the Chat session they came from. Signing out, revoking that session, disabling the account or losing workspace access ends preview access within a few seconds, and closes open preview WebSockets within a minute. The cookie lasts an hour (was eight), renewed by page loads while the sign-in lasts, and is named `__Host-obleth_preview` on https.
- **Only Chat can open a preview.** A preview ticket is no longer a link: Chat's page redeems it with a request the browser marks as coming from Chat's origin, so a ticket sent to someone else, or posted by any other page, opens nothing. Chat's content security policy now lets it connect to preview hosts for this.
- **Other pages cannot send changes into a preview.** Previews share a site, so browsers send one preview's cookie with requests from another. The proxy now refuses requests other than GET, HEAD and OPTIONS unless they come from the preview itself (`Sec-Fetch-Site: same-origin`, or a matching `Origin`). Preview pages get `Referrer-Policy: same-origin` (was `no-referrer`) so their own form posts keep their origin.
- **Preview pages check the pod once, not once per asset.** Requests arriving together share a single check, and while the API server does not answer, a pod confirmed in the last minute is still served instead of every asset failing with 503. The sign-in behind a preview is likewise checked at most every few seconds.
- **Preview apps' cookies stay on their own host.** The proxy strips `Domain` from cookies a dev server sets and drops any named like Chat's own (`obleth_preview`, `__Host-obleth…`, `__Secure-obleth…`).
- **Workspace file tools stay in the project.** Paths with `..` are refused.
- **Downloading a project no longer wakes its workspace by itself.** An asleep, starting or failed workspace, or one whose pod is gone, answers 409 with a message saying which; the download button in the workspace bar shows only while the workspace runs. The Download button on a reply's result card wakes the workspace first, then downloads.
- **Starting and checking a dev server reach only the workspace's own pod**, confirmed the same way as previews.
- **Research runs stop when their owner loses workspace access** or the account is disabled, instead of running on in the workspace.
- **Uploaded files never open as web pages.** Knowledge files, attachments, generated images and workspace files open in the browser only when they are PDFs or PNG, JPEG, GIF or WebP images. Anything else downloads, with text of any kind (HTML, XML, JSON, code) sent as plain text. Knowledge files are stored with a type taken from their name, never the one the browser sent. Documents stored by earlier versions with the uploader's type are sent by Chat itself as downloads rather than through a storage link, so they are safe whether or not the storage honours a link's type. Workspace SVGs still show in the file viewer but download when opened directly.
- **A document that expands enormously no longer exhausts memory.** Word and PDF text is read as the extractor produces it, and the extractor is stopped as soon as its output passes the 64 MiB limit, so a small compressed file that inflates to gigabytes fails with "more text than Chat can index" instead of taking down the server.
- **Signing out another device ends the API tokens made on it.** Revoking a session, or "Sign out other devices", also revokes the personal API tokens created in those sessions, so a token made with a stolen session ends when the person signs that session out. Tokens made on the current device stay. Tokens made before this release are not linked to a session and need revoking by hand.
- **Redeeming a password reset link ends personal API tokens**, as changing or resetting a password already did. Every path that resets someone's sign-ins (password change or reset, reset link, an administrator revoking sessions, changing the role or disabling the account) now ends sessions and tokens together.
- **Forking a shared chat no longer copies its owner's instructions.** The fork keeps the model and sampling settings; the owner's system prompt, which the viewer never saw, no longer steers the viewer's chat and tools.
- **Shared chats no longer show the owner's instructions on each reply.** Viewers see the model and sampling settings and the tools' steps, not the system prompt. The share dialog now says that tool steps and what they returned are visible.
- **Failed sign-ins from other addresses can no longer lock someone out.** The per-account limit of 50 failures a minute now stops only addresses that are failing on that account themselves. An address with no failures there in the last 10 minutes counts against a separate limit of 50 a minute per account instead, so rotating through many addresses still adds at most 50 guesses a minute. Each address keeps its own limit of 10. An address that is refused stays failing until it has been quiet for 10 minutes since its last counted attempt; retrying while refused does not restart that time. Chat logs when an account reaches either limit.
- **The web app offers a fresh sign-in only when a token needs one.** Other refusals when making a token, such as a pending password change, show their own message.

**Upgrading**

- Sign-in text: `login.eyebrow`, `footer`, `welcomeTitle` and `welcomeDescription` are no longer used, and `login.title` is now the heading above the form. Review a customized `site.config.js` before upgrading.
- Open previews need reopening once: earlier preview links and cookies are refused.
- Migration `043` runs when Chat starts: it adds `api_tokens.session_id`.

## v0.5.1

Fixes the browser: opening the Browser tab now starts it.

**Fixed**

- **The Browser tab starts a sleeping browser.** The web app asks with `?wake=1`, which the server refused with 400 before doing anything, so the browser only started when the model used it. The live view now accepts `1` and `true`.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.5.1`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.5.1`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.5.1`

Pin them by digest in production.

## v0.5.0

Chat sets itself up with one command, on Docker or Kubernetes, and workspaces come with the Kubernetes install.

**New**

- **`./setup.sh docker` and `./setup.sh kubernetes`.** They ask for the address people use and the gateway's URL, generate every password and Chat's signing key, start Chat, and print the administrator password and the issuer entry the gateway needs. Run again, they keep what they made. Docker works with Docker Compose or Podman Compose and brings PostgreSQL. On Kubernetes, without `--database`, PostgreSQL 17 with pgvector runs in the namespace (`deploy/k8s-postgres.yaml`). See [Install Chat](https://www.obleth.com/docs/run-chat/install).
- **Workspaces come with `deploy/k8s.yaml`.** It gives Chat a ServiceAccount and a Role for pods, exec and volumes in its own namespace, and NetworkPolicies so only Chat reaches workspace and browser pods and they reach only DNS and public HTTP(S). The namespace enforces Pod Security `restricted`. Nothing else needs setting. See [Enable workspaces](https://www.obleth.com/docs/run-chat/enable-workspaces).
- **Workspaces turn on by themselves.** When Chat runs in Kubernetes with a service account token, workspaces run in its namespace (or `OBLETH_CHAT_SANDBOX_NAMESPACE`). The workspace and browser images default to this release's, and the browser comes with workspaces.
- **Chat says why workspaces are off.** At startup it asks the API server what its service account may do. If something is missing, **Settings › Administration › System** and `/api/admin/health` give the reason.

**Changed**

- Chat caps its workspace and browser pods itself, at `OBLETH_CHAT_SANDBOX_SLOTS` (8), because a ResourceQuota in a shared namespace would count Chat's own pods too.
- `deploy/k8s.yaml` mounts the gateway key at `/run/secrets/obleth-chat/`. Mounted over `/run/secrets`, it hid the service account token's mount point and Chat didn't start.
- Workspace pods may reach DNS on any address, so DNS works whatever the cluster runs, including node-local caches.
- Images are named in full (`docker.io/pgvector/pgvector`), so Podman pulls them.
- A workspace's notes (starting, asleep, no files yet) sit in the middle of the Files and Git panes.

**Limits**

- Workspaces and the browser run only on Kubernetes.
- Previews need a wildcard DNS name and certificate. Without one, everything else works.

**Upgrading**

- Kubernetes installs that mount a service account token now try workspaces. Without the Role, they stay off and say why; `OBLETH_CHAT_WORKSPACES=off` turns them off.
- The browser comes with workspaces unless `OBLETH_CHAT_BROWSER=off`.
- `deploy/k8s-workspaces.yaml` no longer creates the ServiceAccount (now in `deploy/k8s.yaml`) or the `gvisor` RuntimeClass (see the gVisor steps in Enable workspaces). Applying it doesn't remove existing ones.
- If you apply the new `deploy/k8s.yaml`, anything else you run in Chat's namespace must meet Pod Security `restricted`.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.5.0`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.5.0`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.5.0`

Pin them by digest in production.

## v0.4.5

Workspaces no longer need a preview domain.

**Changed**

- **Previews are optional.** Without `OBLETH_CHAT_PREVIEW_DOMAIN`, workspaces, the terminal, files, code and the browser work as before. The Preview tab and open-preview buttons are hidden, and Chat reports `previews: false` in its capabilities. Setting the domain later turns previews on with nothing else to change. See [Enable workspaces](https://www.obleth.com/docs/run-chat/enable-workspaces).

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.5`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.5`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.5`

Pin them by digest in production.

## v0.4.4

Each known embedding model brings its own prefixes, so choosing one in Settings needs nothing else.

**New**

- **Prefixes follow the model.** Choosing Qwen3-Embedding, EmbeddingGemma, Nomic Embed Text, E5, BGE English or mxbai-embed-large uses the search and passage prefixes its makers publish. Settings fills them in when you change the model, and keeps prefixes you edited. A prefix comes from your choice in Settings, then the environment variable while the default model is in use, then the model's own. Any other model gets none unless you set one. See [Choose the embedding model](https://www.obleth.com/docs/run-chat/enable-knowledge#choose-the-embedding-model).

**Upgrading**

- With the default `qwen3-embedding-8b`, the prefixes stay the same, so nothing is indexed again.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.4`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.4`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.4`

Pin them by digest in production.

## v0.4.3

Lets administrators choose Knowledge's embedding model in Settings, and re-indexes everything when it changes. Migration `042` runs when Chat starts.

**New**

- **Embedding model in Settings.** **Settings › Administration › System › Knowledge search** chooses the embedding model, from the embedding models your gateway lists, and the text put before searches and passages. `OBLETH_CHAT_EMBEDDING_MODEL`, `OBLETH_CHAT_EMBEDDING_QUERY_PREFIX` and `OBLETH_CHAT_EMBEDDING_DOCUMENT_PREFIX` become the defaults. Every replica follows a change at once.
- **Automatic re-indexing.** Vectors from different models can't be compared, so when the model or a prefix changes, from Settings or the environment, Chat indexes every document and saved item again. It asks before it starts and shows how much is left. Until it finishes, search finds only what has been redone.

**Fixes**

- A document queued again while it was being indexed is indexed once, by the newer run.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.3`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.3`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.3`

Pin them by digest in production.

## v0.4.2

Keeps people's files in object storage when a bucket is configured, instead of in Postgres, and lets Knowledge use any embedding model. Migration `041` runs when Chat starts.

**New**

- **Files in object storage.** With `OBLETH_CHAT_S3_*` set, chat attachments and generated images go to the bucket that Knowledge uses, under `attachments/` and `images/`, and the database keeps only each file's key. Without a bucket they stay in Postgres as before. Existing files are moved into the bucket in the background, and stay readable while that happens. Chat still serves every file itself, so browsers never need to reach the bucket. See [Where files are kept](https://www.obleth.com/docs/run-chat/enable-knowledge#where-files-are-kept).
- **Any embedding model.** `OBLETH_CHAT_EMBEDDING_QUERY_PREFIX` and `OBLETH_CHAT_EMBEDDING_DOCUMENT_PREFIX` set the text that goes before each search and each stored passage, in the form the model expects. Unset, they keep the Qwen3-Embedding defaults. Models with fewer than 1,024 dimensions, such as EmbeddingGemma, work as well: their vectors are padded with zeros. Changing the model or its prefixes needs the documents uploaded again. See [Enable Knowledge](https://www.obleth.com/docs/run-chat/enable-knowledge).

**Upgrading**

- With a bucket configured, back it up alongside the database: it now holds attachments and generated images as well as Knowledge documents.
- After the move, run `VACUUM FULL attachments, generated_images;` in a quiet period to return the freed space to the disk.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.2`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.2`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.2`

Pin them by digest in production.

## v0.4.1

Adds personal API tokens and an MCP endpoint, so an MCP client such as Claude Code can start and follow your chats, and fixes how replies recover from gateway and connection errors. Migration `040` runs when Chat starts.

**New**

- **API access.** In **Settings › Personal › API access**, make a token for your own MCP client. It connects to `<your Chat>/mcp` and can list models, start chats, send the next message, check on a running chat and stop it. A token is shown once, stored only as a hash, lasts up to 30 days, reaches only its owner's chats and can't change the account.
- **Stacked site name.** A line break in `siteName` (`'Research\nComputing'`) stacks the name beside the logo. The browser title, footer and sign-in text keep it on one line. The site name is a little larger on the sign-in page.
- **Work card.** The live card stays one size and shows the file being written, a command's output or the thinking. Cluster steps go through Inspect, Approve, Apply and Verify (Submit, Watch and Results on Slurm), and worker agents show as a graph of the lead, each worker and their reports. The thinking row stays closed until you open it.

**Fixes**

- A reply carries on when the connection to the model drops partway through a step, and a step is tried again when the gateway reports a server-side failure. When the gateway stops a reply with an error, Chat says why.
- Tool calls that a model writes as text are run, including Llama 4's pythonic calls and calls whose JSON holds raw newlines.
- A tool step that thinks for too long without writing is stopped and asked again at low effort.
- Models ask their questions through `ask_user` rather than in the reply, and ask for a workspace instead of saying they can't run code. A workspace reply that changed files and ran nothing since checks its work before answering.
- The Kubernetes example creates the namespace before its Secrets and shows where `OBLETH_CHAT_TRUSTED_PROXY` goes.

**Images**

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.1`
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.1`
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.1`

Pin them by digest in production.

## v0.4.0

The first release of obleth-chat: a self-hosted chat app for the people who share an [obleth](https://github.com/thediymaker/obleth-gateway) gateway. Each person signs in with their own account, their chats are stored in your Postgres, and every request reaches the gateway as that person, so fair share, budgets and usage apply per person without a shared API key.

> **Installing.** Chat needs PostgreSQL 17 with pgvector and an obleth gateway that trusts Chat's signing key. Workspaces and the browser also need a Kubernetes namespace for their pods. Migrations `001` to `039` run when Chat starts. See [Install](https://www.obleth.com/docs/run-chat/install) and [Features and what they need](https://www.obleth.com/docs/run-chat/features).

**Images**

Multi-arch (amd64 and arm64) images are published with this release:

- `ghcr.io/thediymaker/obleth-chat/chat:v0.4.0`: Chat, the web app and its API.
- `ghcr.io/thediymaker/obleth-chat/workspace:v0.4.0`: the workspace pod image, with Node.js 22, Python 3 (numpy, pandas, scipy, scikit-learn, JupyterLab, Flask, FastAPI, uv), Rust, Go and C/C++ tools, and Vite and Next.js starters.
- `ghcr.io/thediymaker/obleth-chat/browser:v0.4.0`: the headless Chromium for the browser.

Pin them by digest in production.

**Chat**

- **Conversations.** Pick a model or **Auto**, set the thinking effort, attach files and images, and edit or regenerate with versions you can step through. Branch a chat, export and import it as JSON, and search past chats. Replies keep running on the server when you close the tab, and a reloaded page picks up where the reply is.
- **No output cap by default.** Chat sends no `max_tokens` unless a person or an administrator sets a limit, so the model's own applies. A tool call cut off at a set limit is never run; the model is told to write in smaller parts.
- **Context meter.** The ring on the send button shows how much of the model's context the chat uses, sized by the last request a reply sent.
- **Sharing.** Share a chat read-only and live with a group, or with anyone signed in who has the link. Viewers can fork it into a chat of their own.
- **Questions and approvals.** A model with tools can ask you multiple-choice questions, and anything that changes state outside Chat waits for **Allow** or **Deny**.

**Knowledge and projects**

- **Knowledge collections.** Upload PDFs, Word files, HTML, Markdown and code. Replies cite the passage they used, and each citation opens the source at that page. Collections can be shared with a group.
- **Library.** Save code blocks, replies and notes, and ask the model to remember something.
- **Projects and groups.** Give a set of chats shared instructions, collections and saved items, and share the project with a group, such as a class.
- **Project code.** A project keeps one git history, synced from its chats' workspaces, with who changed each line. Compare versions, undo a commit, start a chat from any commit, and take the code out as a zip or pushed to GitHub. A conflict pauses Sync so you, or the model, can combine both sides.
- **Start from a repository.** A new project can start from a public or private repository: Chat copies its default branch with its history, and everything builds on it.

**Workspaces and tools**

- **Workspaces.** The model writes, runs and fixes code in a private Linux pod, and the app appears in a live preview beside the chat, with Files, a Terminal and checkpoints. Any web server shows in the preview, and terminal apps run in the Terminal tab.
- **Explore a repo.** Paste a public repository's address and ask for a tour or a review: the model clones it into the workspace to read.
- **Worker agents.** The model hands parts of a task to faster worker models, which administrators choose, that run at the same time and report back. Workers only read; they never use connections or clusters.
- **Browser.** The model searches and reads pages in a real Chrome tab you watch beside the chat. Take over to sign in; it sees nothing until you hand back.
- **Connections.** MCP servers, Slurm clusters and Kubernetes clusters become tools that act as your own account.
- **Research.** A project can run a tree of measured experiments, each idea on its own git branch, with a chart of every run.
- **Auto mode.** A chat can run unattended until the work is done, with no step or time limit.

**Administration**

- Publish gateway models, choose who sees them, set generation defaults and lock parameters, and mark worker models.
- Daily request and token allowances per person, enforced across replicas.
- Local accounts with invitations, or OIDC sign-in with groups mapped from your identity provider.
- Site name, logo, sign-in and home page text, and footer links, set in one file without rebuilding.
