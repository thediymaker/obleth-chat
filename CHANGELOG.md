# Changelog

Each release's section is also its GitHub Release notes.

## v1.1.0

**New**

- **Memory.** Chat learns lasting things about each person across chats: their role and field, what they work on, the tools they use, their goals and how they like answers. After a reply, a model the administrator chooses reads the person's new messages, as them, and adds, updates or forgets short memories, each linked to the topics it is about. It never keeps health, religion, politics, finances, grades, secrets or anything about other people, and temporary chats are never read.
- **Memories in replies.** A reply gets the memories that fit the message, plus the ones the person pinned. Under the reply, "Remembered 3 things about you" lists them with **Forget**, and anything Chat learned from the exchange shows with **Undo**.
- **Memory page.** A graph of you, the topics your memories are about and each memory, which you can drag and zoom, with a list view. Open a memory to edit it, change its topics or kind, pin it so every chat gets it, or delete it. Open a topic to rename it (a name another topic has merges the two), remove it, or forget everything about it. Add memories yourself, turn off using or learning memories, or delete them all.
- **Dropdowns in the theme.** Every dropdown, in Settings, Administration, the Memory page, sharing and project code, now opens a list drawn in Chat's own colors, light or dark, instead of the browser's plain system list.
- **Administration › System › Memory.** Turn memory on, choose the model that keeps memories, and choose whether it starts on for people who haven't chosen. Memories are matched to messages by meaning when knowledge search is set up, and by their words otherwise.

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
