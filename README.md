<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="obleth" height="72">
  </picture>
</p>

<h1 align="center">obleth-chat</h1>

<p align="center">
  A self-hosted chat app for everyone who shares your models.<br>
  Private conversations, document search with citations, workspaces that build apps with a live preview,<br>
  and your Slurm and Kubernetes clusters as tools.
</p>

<p align="center">
  <a href="https://www.obleth.com/docs/chat/introduction"><b>Documentation</b></a> ·
  <a href="https://www.obleth.com/docs/run-chat/install">Install</a> ·
  <a href="https://www.obleth.com/docs/run-chat/connect-gateway">Connect the gateway</a> ·
  <a href="https://github.com/thediymaker/obleth-gateway">obleth gateway</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-BSL%201.1-blue.svg" alt="License: BSL 1.1"></a>
</p>

<p align="center">
  <img src=".github/assets/screenshots/workspace-build.png" alt="A chat building an interactive heat diffusion simulator in a workspace: checkpoints and Show work on the left, the running app in the live preview on the right" width="100%">
</p>

---

obleth-chat runs on your own servers in front of [obleth](https://github.com/thediymaker/obleth-gateway), the fairshare gateway for self-hosted models. Each person signs in with their own account and their chats are stored in your Postgres. Every request reaches the gateway as that person, so fair share, budgets and usage apply per person, and nobody handles a shared API key.

## What it does

<table>
  <tr>
    <td width="50%" valign="top">
      <h3>Everyday chat</h3>
      Pick a model or <b>Auto</b>, set the thinking effort, attach files and images, and edit or regenerate with versions you can step through. Responses keep running on the server when you close the tab.
      <br><a href="https://www.obleth.com/docs/chat/conversations">Conversations →</a>
    </td>
    <td width="50%" valign="top">
      <h3>Knowledge</h3>
      Upload PDFs, Word files, HTML, Markdown and code into collections. Replies cite the passage they used, and each citation opens the source at that page.
      <br><a href="https://www.obleth.com/docs/chat/knowledge">Knowledge collections →</a>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <h3>Workspaces</h3>
      Describe an app and the model writes, runs and fixes it in a private Linux pod. The running app appears in a preview next to the chat, with files, a terminal and checkpoints.
      <br><a href="https://www.obleth.com/docs/chat/workspaces">Workspaces →</a>
    </td>
    <td width="50%" valign="top">
      <h3>Connections</h3>
      MCP servers, Slurm clusters and Kubernetes clusters become tools, acting as your own account. Anything that changes state waits for you to Allow or Deny it.
      <br><a href="https://www.obleth.com/docs/chat/connections">Connections →</a>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <h3>Projects and groups</h3>
      Give a set of chats shared instructions, documents and saved notes, and share the project with a group, such as a class.
      <br><a href="https://www.obleth.com/docs/chat/projects">Library, projects and groups →</a>
    </td>
    <td width="50%" valign="top">
      <h3>Research</h3>
      Run a tree of measured experiments: each idea is a git branch, every run uses the same command, and a chart tracks the best result.
      <br><a href="https://www.obleth.com/docs/chat/research">Research →</a>
    </td>
  </tr>
</table>

<table>
  <tr>
    <td width="50%"><img src=".github/assets/screenshots/knowledge-chat.png" alt="An answer citing a cluster handbook, with the passage it used and Open at page"></td>
    <td width="50%"><img src=".github/assets/screenshots/cluster-chat.png" alt="The model reading a Slurm cluster's partitions, node and queue through its tools"></td>
  </tr>
  <tr>
    <td align="center"><sub>Answers cite your documents, page by page</sub></td>
    <td align="center"><sub>Your clusters, as tools that act as you</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src=".github/assets/screenshots/questions.png" alt="The model asking where a weekly benchmark should run, with a recommended option"></td>
    <td width="50%"><img src=".github/assets/screenshots/approval.png" alt="A Kubernetes apply waiting for Allow or Deny, with the manifest the model wants to apply"></td>
  </tr>
  <tr>
    <td align="center"><sub>The model asks before it plans</sub></td>
    <td align="center"><sub>Nothing changes until you allow it</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src=".github/assets/screenshots/project.png" alt="A project page with instructions for every chat, its collections and its chats"></td>
    <td width="50%"><img src=".github/assets/screenshots/clusters.png" alt="A Slurm cluster seen live: CPUs and memory in use, partitions, nodes and the queue"></td>
  </tr>
  <tr>
    <td align="center"><sub>Projects share instructions and documents</sub></td>
    <td align="center"><sub>Clusters, live, as your account sees them</sub></td>
  </tr>
</table>

Administrators publish models and choose who sees them, set generation defaults and daily allowances, manage accounts and groups, and can add OIDC sign-in.

## Quick start

You need Rust stable and PostgreSQL 17 with [pgvector](https://github.com/pgvector/pgvector). Node.js 20.12+ is optional; it runs the wrapper that loads `.env`.

```bash
cp .env.example .env
# Set OBLETH_CHAT_DATABASE_URL, OBLETH_CHAT_ORIGIN and OBLETH_CHAT_BOOTSTRAP_PASSWORD
npm run start:api
# http://localhost:8085, sign in as admin
```

Or with Docker Compose, which brings its own Postgres:

```bash
docker compose --env-file .env -f deploy/compose.connected.yaml up --build -d
```

Accounts and chats work right away. To list models and stream replies, [connect the gateway](https://www.obleth.com/docs/run-chat/connect-gateway). The [install guide](https://www.obleth.com/docs/run-chat/install) covers Kubernetes, the first sign-in and production settings.

## Branding and appearance

Edit [web/site.config.js](web/site.config.js) and reload the page. This public, commented configuration controls the site name and subtitle, assistant name, browser title and description, optional logo and favicon, greeting and assistant icons, and the sign-in and home page text. You can also view the deployed file at `/site.config.js`. No build is needed when editing the served file; never put secrets in it.

- Set `logo.light` and `logo.dark` to local image paths such as `./assets/custom/logo-light.svg`. One image works in both themes; leave both blank for a text-only brand. The default favicon is a neutral chat bubble.
- Choose built-in icon names from [web/icons.js](web/icons.js), such as `chat`, `book`, or `sun`; set an icon to `''` to hide it.
- Text is plain text, with `\n` for a line break. Set optional text, including the sign-in and home page footers, to `''` to hide it.
- Set `appearance.defaultTheme` to `system`, `light`, or `dark`. People can switch appearance on the sign-in page or in Settings; their browser remembers their choice.

For Docker, copy the configuration and place your images in a host directory:

```bash
mkdir -p branding/assets
cp web/site.config.js branding/site.config.js
```

Add these mounts under the `obleth-chat` service in `deploy/compose.connected.yaml` (paths are relative to that Compose file):

```yaml
volumes:
  - ../branding/site.config.js:/app/web/site.config.js:ro
  - ../branding/assets:/app/web/assets/custom:ro
```

Recreate the container after adding the mounts. Subsequent branding edits only need a browser reload. Keep image URLs local to the app, for example `./assets/custom/logo-light.svg`; externally hosted images are not supported by the site's security policy. When serving a built `dist/` directory, edit its `site.config.js` and assets, or rebuild after editing `web/`.

## Repository

| Path | What's there |
| --- | --- |
| `api/` | The Rust server: auth, conversations, gateway streaming, Knowledge, workspaces, connections, research |
| `web/` | The browser app, plain JavaScript modules with no build step |
| `migrations/` | Postgres migrations, applied at startup |
| `deploy/` | Dockerfiles, Compose, a Kubernetes reference manifest, the workspace image, Prometheus samples |
| `tests/` | Rust integration tests and Node unit tests |
| `server/` | A static design preview with sample data (`npm start`) |

## Development

```bash
npm test                       # web unit tests
cargo check --all-targets
# Rust tests need a disposable Postgres server; they create their own databases
DATABASE_URL=postgresql://… TEST_DATABASE_URL=postgresql://… cargo test --all-targets
```

## Credits

Space Grotesk is bundled under the SIL Open Font License (`web/assets/FONT-LICENSE.txt`). markdown-it and its dependencies are bundled with their notices in `web/vendor/markdown-it/`. Research mode follows the playbook of [OpenResearch](https://github.com/alphaXiv/OpenResearch) (MIT).

## License

[Business Source License 1.1](LICENSE). Source-available; each release converts to [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0) four years after publication. Free for internal, academic, research, and educational use. You may not offer obleth-chat as a hosted service or distribute a competing product derived from it until the Change Date. Contact the maintainers for commercial licensing.
