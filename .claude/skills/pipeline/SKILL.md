---
name: pipeline
description: A disciplined project development pipeline in stages 0–10 — context load → idea → plan + criteria → risk spikes → contracts → vertical slice → tasks → implementation → audit → feedback → release. Use when the user starts a new project, says "запусти пайплайн", "по нашему процессу", "следующая стадия" ("run the pipeline", "our process", "next stage"), or when the project has docs/pipeline/state.md and work needs to continue.
argument-hint: "[stage number or \"next\"]"
---

# Pipeline: a project from idea to release

Today: !`date +%F`. **Take every date in the artifacts from here**, not from memory —
you don't know your current date.

Reply to the user in the language of their message.

Eleven stages with **hard gates** between them. You lead the user through the stages,
materialising each one as a file. The files are the only source of truth: the session's
context dies, `docs/pipeline/` lives on.

```
0. context load    — the project brief is loaded, not retold
1. idea            — what and why, in 3–5 sentences
2. plan + criteria — steps + an explicit "done = ..."
3. risk spikes     — only what's truly unknown, throwaway code
4. contracts       — models, signatures, response schemas, error format
5. vertical slice  — one path, end to end
6. tasks           — atomic, each with its own definition of done
7. implementation  — one task per session
8. audit           — in a fresh context: tests + manual checks
9. feedback        — audit findings → back into the brief + decision log
10. release        — shipping: pre-launch checks, access, backups, rollback
```

Stages 0–9 are the development cycle; it repeats. Stage 10 isn't done every round,
only before a real publication.

## Artifacts

Everything lives in `docs/pipeline/` at the project root:

| File | Stage | Nature |
|---|---|---|
| `state.md` | all | the current stage + gate status. Update it **every** time |
| `brief.md` | 1, 5, 9 | a living document: what we build, why, constraints, stack, how to run |
| `plan.md` | 2 | steps + acceptance criteria (verifiable) |
| `spikes/` | 3 | throwaway code + `spikes/FINDINGS.md` with the answers |
| `contracts.md` | 4 | data models, signatures, schemas, config, errors |
| `tasks.md` | 6 | atomic tasks with DoD checkboxes |
| `audit.md` | 8 | the fresh-context audit report |
| `decisions.md` | 9 (append) | the decision log: date, decision, alternatives, why |
| `release.md` | 10 | what was shipped, where, how it was checked, how to roll back |
| `reports/T-xx.md` | 7 (orchestrated) | a child session's task report: DoD evidence, findings, proposals |

Templates are in `references/templates.md`. Stage details are in `references/stages.md`.
Read the relevant section of `stages.md` **before** starting a stage, not from memory.

## The gate rule (never break it)

At the end of every stage:
1. Write the artifact to disk.
2. Update `state.md`.
3. Show a **condensed** summary: the gist + what was decided. Not a retelling of the file.
4. Ask an explicit question about moving on, and **stop**. You don't start the next stage
   until the user has said "yes / next / go".

Exception: the user explicitly said "don't ask, run it to the end". Then you go straight
through, but you still write the artifacts and show their list at the end.

## The stage-zero rule

`brief.md` **is loaded, not retold.** Once you've read it, work with that knowledge silently.
Retelling the brief to the user is forbidden: they wrote it. At most one line:
"brief loaded: <project>, stage N, next task T-xx".

## Session start

1. Is there a `docs/pipeline/state.md`? → stage 0: read `brief.md`, `state.md` and the current
   stage's artifact. One line of confirmation → continue from the recorded stage.
2. No? → a new project: create `docs/pipeline/`, scout the environment (stage 0),
   move to stage 1.
3. The user named a stage explicitly ("let's do contracts") — jump there, but first read
   the artifacts of the previous stages. Don't invent context you haven't read.

## A project that already has code (brownfield)

The pipeline is written for a blank slate. In an existing project the stages are the same,
but three of them work in reverse — you don't design, you **extract**:

- **1 idea.** The brief is written from the facts: what the system does **now**, and in a
  separate section, what we want to change. Don't rewrite the author's intent; you don't know it.
- **4 contracts.** Contracts are read out of the code: real signatures, real models, the
  real config format. A mismatch between "as written" and "as intended" is a finding; it
  goes into `decisions.md`, not into a silent "fix".
- **5 vertical slice.** The slice already exists. Instead of writing one, **run** the
  existing end-to-end path and show the output. If it doesn't run, that's the first task,
  not a reason to move on.

The scope narrows to the change: we run the pipeline over the part of the system we touch,
not the whole repository. A full inventory of someone else's code is separate work, and it
isn't done "along the way".

## Stages (briefly; in full in references/stages.md)

**0. Context load.** A returning session: load the brief + state silently. A new project:
scout with tools — what's in the repo, runtime versions, git/CI/tests, OS specifics.
Don't ask the user what you can check yourself.

**1. Idea → brief.md.** Draw out the essence with a batch of questions (AskUserQuestion, not
one at a time). Who the user is, what pain we solve, what counts as success, what is
explicitly out of scope, hard constraints. The result is 3–5 sentences confirmed by the
user, plus researched external dependencies (docs, licences, ToS). It goes into `brief.md`.

**2. Plan + criteria → plan.md.** 3–7 large steps. Each gets a "done = ..." in the form
"given → when → then". Test each criterion with the question "how will I check this without
asking for an opinion?". No answer — rewrite it. Non-goals go separately.

**3. Risk spikes → spikes/.** Only what we don't know and what could bring the plan down.
One spike — one yes/no question + facts. The code in `spikes/` is **throwaway**: don't
refactor it; production code doesn't import it. The answers go into `spikes/FINDINGS.md`.
A spike that kills an assumption sends us back to stage 2 — that's the spike's success,
not its failure.

**4. Contracts → contracts.md.** Data models, public signatures, storage and response
schemas, the config format, the error taxonomy. Write them as code, not prose. There's no
implementation — only shapes. The check: from the contracts you can write stubs, and they
fit together by type. **Secrets and environment** belong here too: which keys are needed,
where they come from, what's in `.env.example`, what `.gitignore` covers. No secret is ever
written into a contract by value, only by variable name.

**5. Vertical slice.** One end-to-end path: input → processing → output, actually working.
The narrowest possible, with everything unrelated hard-coded. Real dependencies, not mocks.
**Run it and show the output.** The stage isn't closed until the slice has run live.
Once it works, immediately write the "How to run" section of `brief.md` with the commands
that just worked. Future sessions and the stage-8 auditor will work from them.

**6. Tasks → tasks.md.** Atomic: one task's size = one meaningful commit. Each has files,
what we do, a DoD and dependencies. The order in the file = the order of execution.

**7. Implementation.** **One task per session.** Done → checked the DoD for real (a run or
a test, not "looks right") → ticked `[x]` → a 1–3 line report → stopped and asked. If a task
grows, stop, split it in `tasks.md`, then do it.

**The three-attempt rule.** If the DoD doesn't pass after three tries, stop fixing.
It isn't that the task can't be solved; the task is framed wrongly: knowledge is missing
(→ a spike, stage 3), the contract is wrong (→ stage 4) or the DoD is unverifiable (→ stage 6).
Report to the user what you tried and which of the three cases you see. A fourth attempt
the same way is the most expensive way to spend a session.

**8. Audit → audit.md.** A subagent (Agent, `general-purpose`) with a fresh context.
It didn't see how the code was written — that's the point. It checks the acceptance criteria
**by running things**: tests + manual checks. The prompt follows the template in
`references/stages.md`. You don't run the audit yourself.

**9. Feedback.** Audit findings → back into `brief.md` (bring it in line with what was built)
and into `decisions.md` (append-only, absolute dates). Remaining defects become tasks in
`tasks.md`. The next pass starts from stage 7, not stage 1.

**10. Release → release.md.** Only if the project is actually shipped somewhere. The audit
checked that the code does what it promised; the release checks that it **survives contact
with the outside world**: secrets, access, limits, backups, rollback.
It's run through separate skills, not from memory: `/launch-security` for any application
with a server or other people's data, `/launch-web` for a public website.
Their reports land in `docs/launch/`; P1 findings become tasks in `tasks.md` and are fixed
before shipping; the rest goes to the backlog. In `release.md` you record: what was shipped,
where, which version/commit, how it was checked after shipping, how to roll back and who
has access. The `/launch-*` skills **are invoked by the user** — you don't run them yourself;
say in one line that it's time.

## Routing: who runs a stage

The main loop's model can't be switched from inside — the user sets it with `/model`.
So routing works like this: **thinking stays in the main loop, routine goes to subagents
whose model is pinned in their definition.** Model names live there, not here: the skill
talks about roles and survives model generations changing.

| Stage | Who runs it | Why |
|---|---|---|
| 0 context load | main loop | cheap, a couple of commands |
| 1 idea | main loop | a dialogue with the user, nothing to delegate |
| 2 plan + criteria | main loop, strong model | the project's fate is decided here |
| 3 risk spikes | subagent `pipeline-spike` | self-contained mechanics: script → run → facts |
| 4 contracts | main loop, strong model | architecture; a mistake here costs the most |
| 5 vertical slice | main loop | we test the contracts ourselves, not with someone else's hands |
| 6 tasks | main loop | splitting needs the whole picture |
| 7 implementation | by task type, see below | |
| 8 audit | subagent `pipeline-auditor` | needs a fresh context, not a cheap model |
| 9 feedback | main loop | |
| 10 release | main loop + the `/launch-*` skills | the user invokes the checks |

**Stage 7 — the task's executor is decided at stage 6, not on the fly.** Give every task in
`tasks.md` an `Executor:` field (in a Russian-language `tasks.md` it's `Исполнитель:` with the
values `субагент` / `главный цикл`):

- `subagent` — routine: boilerplate, tests against a finished contract, CLI wiring, parsing
  against a described schema, mechanical edits. You hand it to the `pipeline-task` subagent.
- `main loop` — the task touches architecture, needs decisions or a dialogue with the user.
  You do it yourself. **When in doubt — main loop**: delegation isn't free, the subagent
  rebuilds context from scratch, and on an ambiguous task that costs more than doing it yourself.

What to tell the user about their side: at stages 2 and 4 keep the strongest available
model; at stage 7, with a batch of routine work, they can switch to a fast one. They see the
current names in `/model`, which also has combined modes like "plan with the strong one,
execute with the fast one". Say this **once, at the gate** of the relevant stage, not in
every message.

## Orchestrated mode

When the user wants to talk to one session while the work happens elsewhere ("orchestrate",
"run the tasks in sessions", or the project's `state.md` says `Mode: orchestrated`), this
session becomes the **orchestrator**: it keeps `tasks.md`/`state.md` and the dialogue, and every
task runs in a child session that implements, checks its DoD, commits on its own branch and
writes `docs/pipeline/reports/T-xx.md`. The orchestrator never reads code or diffs; it checks
each DoD by running its command with the output cut to the verdict, then merges the child's
branch (standing permission: merge on pass, never push). Read `references/orchestration.md`
before starting a child — brief and report templates, when tasks may run in parallel, conflicts.
Record `Mode: orchestrated` in `state.md` so the next session continues the same way.

## Code style — applies at every stage

### Comments

A comment explains **why**, not **what**. What the code does is visible from the code.

Write a comment only if it carries knowledge that isn't in the code:
- a non-obvious external constraint ("OLX's `private_business` returns 200 and silently
  doesn't filter — the working parameter is `owner_type`")
- the reason for a workaround and what breaks without it
- where a magic value came from (a link to a spike, a ticket, a measurement)
- a deliberate rejection of the obvious solution and its reason

**Never write:**
```python
# Increment the counter by one
counter += 1

# Function for getting listings
def get_listings(...):

# ---------- HELPERS ----------
```
That's noise: it repeats the code, goes stale first and teaches people not to read the code
itself. Docstrings only where the contract, units or side effects aren't obvious.
A trivial function doesn't need a docstring. **A docstring that restates the function's
name (`"""How long to sleep until the next poll"""` above `seconds_until_next`) is pure noise.**

**Measurable guidelines** (the verbal "match the surrounding code" isn't enough in practice):

- Comments in a file — about **5–8% of the lines of code**. Above 12% is almost certainly
  explaining yourself, not the code.
- **No more than one comment line above a single call.** Three lines of justification above
  one line of code is a commit message, not a comment.
- A spread in density between files of one project is a sign of breakage. If one module is
  at 25% and another at 3%, the train of thought was commented: denser where you thought longer.

Check yourself with the question: **"is this knowledge or my thought process?"** About
someone else's API, about pitfalls, about the reason for a non-obvious choice — knowledge,
keep it. About how you arrived at the solution — process, throw it out.

**Don't explain an absence.** "Reference data doesn't belong here", "there's deliberately no
retry here" is an endless genre: you can append to any line what isn't in it. Comment on
what is there. The exception is when the absence looks forgotten and someone will try to "fix" it.

**Don't retell common knowledge.** "Dependencies in a separate layer so they aren't rebuilt"
in a Dockerfile, "the index speeds up lookups" on CREATE INDEX — anyone reading such a file
knows that. The rule applies outside code too: YAML, Dockerfiles, configs follow the same norm.

In a file with no comments, don't start adding them.

### Git

- **Don't list yourself as an author or contributor.** No `Co-Authored-By`,
  `Generated with`, or mentions of the model in the commit or PR body. The author is the user.
- The commit message is about the substance of the change, in the style the repository
  already uses (look at `git log` before writing the first commit).
- **Commit and push only on explicit request.** "One task = one commit" from stage 6 is a
  measure of a task's **size**, not permission to commit yourself: the task must be such that
  its changes add up to one meaningful commit.
  Exception, orchestrated mode: a child commits its task on its own branch and the orchestrator
  merges it after the DoD check passes (the user's standing permission). Nobody pushes.
- Having closed a task at stage 7, propose a ready commit message in one line of the report.
  The user commits themselves or says "commit". Silence isn't consent.
- Before the first commit, check that `.env`, keys, dumps and build junk didn't get into the
  index. A secret that got into history is cleaned by rotating the key, not by `git rm`.

## How to behave inside a stage

- **Don't run ahead.** At contracts don't write the implementation; at the plan don't write
  contracts. The most common pipeline breakage is "I've already sketched this out while I
  was at it".
- **Don't invent facts.** If you don't know how an external service or library behaves,
  that's a spike (stage 3) or a web search, not a guess in the brief.
- **Record decisions immediately** as a draft line; at stage 9 you'll collect them into
  `decisions.md`.
- **Artifacts are in the user's language**, code and identifiers in English.
