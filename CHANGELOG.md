# Changelog

Each release's section is also its GitHub Release notes.

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
