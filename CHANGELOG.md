# Changelog

Each release's section is also its GitHub Release notes.

## Unreleased

**Fixed**

- **Previews only reach the workspace's own pod.** Before proxying, Chat checks with the API server (at most every few seconds per pod) that the workspace pod is still running at the address on record. A pod that was evicted, deleted or lost with its node is marked stopped instead of its old address being used, which another pod may have been given. The reconciler does the same every minute.
- **Preview access ends with the sign-in.** Preview links and the preview cookie carry the Chat session they came from. Signing out, revoking that session, disabling the account or losing workspace access ends preview access at once. The cookie lasts an hour (was eight) and is named `__Host-obleth_preview` on https.
- **Preview apps' cookies stay on their own host.** The proxy strips `Domain` from cookies a dev server sets and drops any named like Chat's own (`obleth_preview`, `__Host-obleth…`, `__Secure-obleth…`).
- **Workspace file tools stay in the project.** Paths with `..` are refused.
- **Downloading a project no longer wakes its workspace.** An asleep workspace answers 409 and asks to be opened first; the download button shows only while the workspace runs.
- **Research runs stop when their owner loses workspace access** or the account is disabled, instead of running on in the workspace.

**Upgrading**

- Open previews need reopening once: earlier preview links and cookies are refused.

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
