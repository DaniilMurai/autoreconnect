# Orchestrated mode

One session talks to the user and keeps the plan: the **orchestrator**. Every task runs in its
own **child session** (`start_session`, its own worktree). The orchestrator's context is the
scarce resource: it holds the queue and the decisions, never the code.

Used for stage 7 by default when the user asks for it, and for stage 3 (one child per spike)
and stage 8 (the auditor). Stages 1, 2, 4, 6 stay in the orchestrator: they are dialogue and design.

## What the orchestrator reads and writes

Reads: `state.md`, `tasks.md`, `docs/pipeline/reports/T-xx.md`, the 400-character notes the app
sends when a child ends a turn, and the verdict lines of a DoD command (`| tail -5`).
Never reads: source files, diffs, full test output, a child's transcript.
If it needs to know something about the code, it asks a child or a subagent a question and
gets the answer back in a few lines.

Writes: `tasks.md` (ticks, new tasks, split tasks) and `state.md`. **Only the orchestrator
writes them** — two children ticking the same file in parallel produce a merge conflict on
every merge. Children propose changes in their report.

## Picking what runs

1. Before starting children, commit the orchestrator's branch: a child's worktree is cut
   from its tip, uncommitted edits are not there.
2. Ready = all `Depends on` are ticked and merged.
3. Run in parallel only ready tasks whose `Files:` do not overlap. Two tasks that touch the
   same file run one after another, even if neither depends on the other (lesson: T-01 and
   T-03 both changed the `JsonlSink::open` call in `main.rs`).
4. At most 3 children at once. More is more notes, more merges and more "needs you" at once.
5. `Executor: subagent` → child on a fast model. `Executor: main loop` → child on the strong
   model; the user may open and steer it directly.

## The child brief

Short — the child reads the docs itself. Each `start_session` prompt:

```
Pipeline task T-xx in <repo>. You are a child of the orchestrator session; you do this one task.
Read first: docs/pipeline/contracts.md, docs/pipeline/tasks.md (T-xx), docs/pipeline/brief.md ("How to run").
Do: <the task's "Do" line>. DoD: <the task's DoD, verbatim>.
Rules:
- Follow the pipeline skill's code style. Do not edit tasks.md or state.md.
- Safety: <project rules, e.g. AUTORECONNECT_DRY_RUN for every run that isn't a fault-injection test;
  sudo/pf/router steps are the user's>.
- Three attempts: if the DoD fails three times, stop and report which case it is (knowledge, contract, DoD).
- When the DoD passes: commit on your branch (one commit, the repo's message style, no co-author lines),
  then write docs/pipeline/reports/T-xx.md from the template and commit it too.
- If you need the user (sudo, hardware, a decision): write the report with status "needs-user" and stop.
```

## The report file

`docs/pipeline/reports/T-xx.md`, written by the child, at most ~30 lines:

```markdown
# T-xx — <title>
Status: done | needs-user | blocked
Branch: <branch>  Commit: <sha>
## DoD evidence
- <command> → <verdict line(s)>
## Needs from the user (only if needs-user)
- <the exact action, a ready-to-paste command block, what to reply>
## Findings
- <what the plan/contracts did not know; one line each>
## Proposed changes to tasks.md / contracts.md
- <new task, split, DoD change — the orchestrator decides>
```

## When a child reports

1. Read the report file from the child's branch (`git show <branch>:docs/pipeline/reports/T-xx.md`).
2. `needs-user` → pass it to the user in one line plus the child's link and the command block;
   the answer goes back with `SendMessage` to the child (quote the user's words).
3. `done` → **independent check**: run the DoD command in the child's worktree yourself,
   output cut to the verdict (`cargo test ... 2>&1 | grep 'test result'`). A live DoD (sudo,
   router, another person) counts as checked only with the user's own confirmation.
4. Check passes → **merge on pass** (standing permission from the user, 2026-11-03):
   `git merge --no-ff refs/heads/<branch>` into the orchestrator's branch. Never push.
   Conflict → `git merge --abort`, send the conflict list to the child, it rebases onto the
   orchestrator's branch and reports again.
5. After the merge: tick the task, apply accepted proposals to `tasks.md`, one line to `state.md`,
   then pick the next ready tasks.
6. Report to the user per batch, not per child: a table of tasks, status, commit.

Check fails → the task goes back to the same child with the failing line, not to a new one.

## Keeping the orchestrator alive

- Every 3–4 merged tasks: rewrite `state.md` so that a fresh orchestrator can continue from it
  alone (current batch, running children with their session ids, open user asks).
- If the orchestrator's context is long anyway: tell the user, then a new orchestrator session
  starts from `/pipeline` and `state.md`. Notes from running children still go to the old
  orchestrator, so the new one finds them by the session ids in `state.md` and by their report files.
- The environment changes under long runs (reboots, OS updates, another network). A child that
  sees its environment differ from `brief.md` reports it as a finding instead of adapting silently.
