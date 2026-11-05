# Certification

100 cases over the ways a model works inside Chat: answering, formatting, asking with
the question card, offering and using a workspace, research and app offers,
knowledge, the browser, Kubernetes and Slurm connections, worker agents, and
several turns. Each case runs as a real chat through Chat's MCP endpoint, so a model
sees the same prompts and tools people get. `cases.yaml` explains the checks.

`cases-people.yaml` is a second set of 100, written the way people actually type:
short, vague or missing context. It checks that a model picks the right thing from
everyday wording, and holds back when no tool or offer fits. Run it with
`--suite cases-people.yaml`.

## Set up the test account

1. An administrator account with workspaces and the browser allowed.
2. A project named **Reef Lab** whose collection holds the files in
   `fixtures/reef-lab/`.
3. Connections, named as in `config.yaml`:
   - a Kubernetes connection whose default namespace holds Deployment `reef-api`
     (2 replicas, healthy) and Deployment `reef-worker` (1 replica, crash-looping
     because `REEF_DB_URL` is not set), with **Ask before changes** on.
   - a Slurm connection for an account with some job history. The submit and
     cancel cases stop at the approval, so nothing runs.
4. A personal API token with write scope: **Settings › Personal › API tokens**.

## Run

```bash
pip install pyyaml
CERTIFY_TOKEN=obc_... ./run.py --base https://chat.example.edu \
  --models glm-5-3,deepseek-v4-1 --judge deepseek-v4-1 --judge-for deepseek-v4-1=glm-5-3 \
  --out results/
```

`--cases` runs only some cases, by id or area (`--cases C01,knowledge`), and
`--suite` picks the case file. Each model
gets `results/<model>.jsonl`, one line per case with its steps, reply, the failed
checks and a link to the chat; `results/summary.md` has the score table.

Each chat a run makes, and each judge chat, is deleted once it is graded, since an
account holds at most 1,000 chats; the record keeps the steps and the reply. `--keep`
keeps them. `cleanup.py` deletes chats titled `certify ...` that an interrupted run
left. `rescore.py` merges a full run with reruns of some cases and grades them all
again with the current checks.

Gold models should pass every case. When one fails a case, the case, a fixture, a
prompt or the harness is wrong until shown otherwise.
