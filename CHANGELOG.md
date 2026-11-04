# Changelog

Each release's section is also its GitHub Release notes.

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
