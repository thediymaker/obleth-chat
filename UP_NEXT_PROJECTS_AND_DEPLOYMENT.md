# Up next: projects, GitHub, and external deployment

**Status:** Partly built. v0.4.0 shipped project code (one git history synced from chats' workspaces), export as a zip or pushed to GitHub, and starting a project from a repository. Deploying to a hosting provider is still exploration; nothing below that is approved or started.

**Updated:** October 24, 2026.

**Purpose:** Capture the direction and design questions for turning generated work into a saved project that its owner can publish through an external hosting provider.

Users create demos, applications, reports, and other work in chat workspaces or through tools connected to Slurm and Kubernetes. They need a way to preserve that work, understand its requirements, optionally manage its code in GitHub, and publish suitable applications.

The agreed direction is to help users deploy into their own provider accounts, with guidance for services such as Cloudflare, Vercel, and GitHub Pages. Operating a public application-hosting service, continuously monitoring hosted applications, and automatically suspending them are outside this proposed scope. External hosting still leaves application permissions, credentials, data handling, and maintenance to address.

The existing [obleth-chat README](README.md) describes Projects as shared instructions, documents, notes, and conversations. The proposed source files, versions, and deployment records need to fit that existing concept. Whether they become project capabilities or a linked application within a project remains open. The flows and defaults below are proposals, not descriptions of current behavior.

The proposed journey is **select work → convert to project → review requirements → optionally connect GitHub → choose a host → complete deployment setup → retain the result and update path**.

1. **Convert selected work to a project.** Offer an entry point from a workspace, a generated artifact, or the results of a connected job. Let the user name the project, choose an owner or existing project, and review included files. Suggest private access. Conversion preserves work without publishing it. Save incomplete projects and identify the requirements still to resolve.

2. **Identify what the project contains.** Distinguish a static site, web application, background service, batch job, report, or dataset. Source code, generated outputs, data, and transient execution state should remain distinguishable. A Slurm result may be a shareable report without being a web application. A Kubernetes service may depend on infrastructure that a proposed host cannot provide. If source is unavailable, describe an output-only project accurately.

3. **Optionally keep the code in GitHub.** Offer Create repository, Connect existing repository, or Set up later. Show the owner or organization, repository visibility, destination branch, and proposed file changes. Suggest a private repository and explicit updates initially. For an existing repository, preserve unrelated work, detect conflicts, and offer a review branch. Automatic synchronization and automatic deployment require separate, visible configuration.

4. **Prepare an external deployment.** Inspect the selected version, identify missing setup, and recommend a compatible host. Start with tailored instructions and prepared configuration where that is sufficient. Explain where users must configure secrets, storage, databases, or other services. Show the destination account, website access, and provider billing responsibility before publication. A private source repository does not establish private website access.

5. **Retain a useful deployment record.** Store the provider, project reference, deployed version, URL, and last verified status. Offer the hosting dashboard link and an update path. Show deployment failures separately from project-saving failures. With a guidance-only handoff, report setup as incomplete or unverified until there is evidence of completion; opening a provider page is not deployment success.

For example, a conversion panel could show:

```text
Convert to project

Name             [Experiment explorer]
Save in          [My projects / Existing project]
Detected content Web app with a backend

Include
[x] Application source and dependency files
[x] Setup instructions
[ ] Generated dataset
[ ] Execution logs

Needs attention
One dependency uses files from this workspace.

[Create project]
```

The project view should make these distinctions understandable:

| Item | What it represents |
| --- | --- |
| Workspace or job | The environment where work was created or executed; it may be temporary. |
| Saved project version | A selected snapshot with enough context to understand and use it. |
| GitHub revision | The code saved to a repository and branch, when connected. |
| Deployment | A particular version published to a particular provider account. |

An example status line is **Project saved · GitHub has pending changes · Published version: 3**. An update to the workspace should not silently update a project or a public site. If an existing Git branch already triggers deployment, explain that consequence before writing to it.

A minimum project record would identify its owner and access policy, included source or output artifacts, origin and version, project type, build/run instructions, runtime and dependency requirements, required configuration names, and external data or storage dependencies. Keep secret values out of source snapshots. Project persistence should survive the original execution environment ending or a hosting connection being removed. Storage location, retention, and quotas still need design.

GitHub should support code history and collaboration without becoming mandatory for every project. GitHub Pages is a separate static-hosting option, with [usage limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits) that affect suitability. For a dedicated connection, evaluate a [GitHub App](https://docs.github.com/en/apps/creating-github-apps/about-creating-github-apps/deciding-when-to-build-a-github-app) with access limited to the needed repositories. Define creation, organization approval, branch permissions, and revocation behavior before selecting exact grants.

Contextual guidance should appear beside a decision and offer an action:

| Trigger | Example guidance | Action |
| --- | --- | --- |
| Selecting files | “This selection includes datasets and execution logs. Review what you want to upload.” | Review included files |
| Making a repository public | “Anyone can read this repository's source and history. Keep it private unless you intend to share them.” | Review visibility |
| Configuring website access | “Repository visibility and website access are separate settings.” | Review website access |
| Finding a possible credential | “This file may contain an API key. Store secrets in your hosting provider's settings.” | Review and exclude |
| Updating a known deployment branch | “Updating this branch will trigger a deployment.” | Use a review branch |
| Finding newer repository changes | “GitHub has changes since your last update. Review them before sending yours.” | Review changes |
| Finding a workspace or cluster dependency | “This project needs files or services from its original environment.” | Resolve dependencies |

Use inline hints for common choices and click-to-open explanations for concepts such as repositories, branches, and environment variables. Support keyboard and touch interaction. Remember dismissed educational hints; resurface warnings when relevant facts change. Reserve blocking states for the affected action when there is a concrete problem, such as a confirmed credential in an upload, unresolved conflict, missing permission, or unsupported deployment requirement. Avoid a sequence of mandatory tutorial popups.

Prepare useful defaults where possible: a README, sensible file exclusions, dependency lockfiles where applicable, and configuration examples containing names without secret values. Readiness checks should report what was checked and what remains unknown. A successful build or scan is not a security certification. If a credential has already leaked, deleting it from an export is insufficient; it needs revocation or rotation. See [GitHub's secret-scanning guidance](https://docs.github.com/en/code-security/concepts/secret-security/secret-scanning).

Connectivity remains an implementation decision:

| Approach | Proposed role | Work still required |
| --- | --- | --- |
| Guided provider setup | Early option: prepare files and give project-specific instructions or setup links. | Compatibility checks, accurate guidance, and an honest handoff/completion state. |
| Git-based publishing | Use a repository as the source for a provider's build workflow. | Repository permissions, branch behavior, configuration, and update ownership. |
| Provider API or CLI | Add direct upload, deployment progress, and updates where worthwhile. | Credential isolation, packaging, durable jobs, retries, and status verification. |
| MCP | Let the assistant invoke supported project and deployment operations. | The same permission, artifact-transfer, and lifecycle controls as other integrations. |

MCP is a viable integration mechanism, but availability and capabilities vary by provider and client. It does not by itself supply a dependable deployment workflow. Keep UI and assistant actions consistent, with credentials outside generated code and untrusted build processes. A deployment should refer to an identified snapshot or revision rather than whatever happens to be in a mutable workspace.

Provider setup buttons also have constraints: the documented [Vercel](https://vercel.com/docs/deploy-button) and [Cloudflare](https://developers.cloudflare.com/workers/platform/deploy-buttons/) template flows use public source repositories. They should not require users to expose private source merely to simplify deployment. Private projects may need an authenticated import, upload, or CLI workflow. Revalidate provider capabilities when implementation begins.

The next work is design and validation:

- [ ] Decide how saved application files and versions fit existing Projects and any future Space concept.
- [ ] Define ownership, access, persistence, retention, and the minimum project record.
- [ ] Specify what conversion captures from a workspace, a connected job, or an existing repository.
- [ ] Sketch conversion, GitHub setup, readiness, and deployment screens, including failure and conflict states.
- [ ] Choose initial supported deliverables and test the proposed handoff against representative apps.
- [ ] Define GitHub snapshots, manual updates, optional synchronization, and deployment-branch behavior.
- [ ] Define contextual hints, file exclusions, secret handling, and proportionate readiness checks.
- [ ] Select an initial provider workflow based on compatibility and user friction; compare instructions, Git workflows, MCP, and direct APIs.
- [ ] Define completion evidence and useful measures: conversion success, setup abandonment, deployment success, and recurring blockers.

A candidate first release would cover selected workspace files, private project versions, optional GitHub export or connection, readiness guidance, and external deployment instructions for static sites and one validated web-app stack. Broader job imports and direct provider integrations would follow evidence from those workflows. This is a scope proposal for review, not a commitment to begin implementation.
