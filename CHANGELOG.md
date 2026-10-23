# Changelog

Each release's section is also its GitHub Release notes.

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
