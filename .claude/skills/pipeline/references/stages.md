# Stages — in full

Read the section for a stage before you start it.

---

## 0. Context load

**A returning session** (there's a `docs/pipeline/state.md`):
read `brief.md`, `state.md` and the current stage's artifact. Then work with that knowledge
**silently**. Retelling the brief to the user is forbidden — they wrote it and don't want to
read it back. The allowed maximum:
`brief loaded: OLX parser, stage 7, next task T-05.`

**A new project** (no files): scout with tools, not questions.
- What's already in the repository — a blank slate or someone else's code?
- Runtime versions, package manager, OS specifics, paths with spaces/Cyrillic.
- Whether there's git, CI, linters, tests.
Ask the user only about what can't be checked with a command.

---

## 1. Idea → brief.md

Goal: turn "I want a thing" into an understood and bounded intent. No code.

Ask in a batch (AskUserQuestion, not one question at a time):
- Who is the user and in what scenario do they run it? (themselves once / on a schedule / other people)
- What pain do we solve — what does the person do by hand now?
- What counts as success? One measurable sentence.
- What is explicitly **out** of scope for this version?
- Hard constraints: platform, budget, privacy, deadline, "local only".

In parallel — research the domain: external APIs/sites/libraries, official docs,
how current the versions are, licences, ToS/robots.txt. If the data is newer than your
knowledge cutoff or there's doubt — WebSearch/WebFetch. Don't write into the brief what
"seems to work that way".

Output: `docs/pipeline/brief.md`. The gist is 3–5 sentences. Show the user the
**"Constraints and risks"** section in full: that's usually where things they hadn't thought
about come up.

Red flag: after the answers the scope is still "well, it does everything" — press with the
question "what's needed in the first working version, and what can come later".

---

## 2. Plan + criteria → plan.md

Steps are large (3–7 of them), not tasks. Tasks come at stage 6.

Acceptance criteria are the stage's main value:
```
AC-3: Given — the search query "iphone 13" and a price filter of 100–500.
      When — we run `app search --query "iphone 13" --price 100:500`.
      Then — stdout has ≥1 listing, each with a price in the range, and a repeated
      run within an hour doesn't return the same listings again.
```
Test every criterion with the question: **"how will I check this without asking for an
opinion?"** No answer — rewrite it.

In a separate section: **non-goals** — what we deliberately don't do in this version.

---

## 3. Risk spikes → spikes/

The selection rule: a spike is written **only** for what we don't know and what could bring
the plan down. Known things ("I can write a CLI with argparse") don't get spikes.

Typical candidates: the behaviour of a protected external site/API, whether a library works
on this OS/version, whether an operation fits in an acceptable time, whether parsing is
accurate enough.

For each one:
1. Phrase the question as yes/no with facts: "Does X return data without authorisation?
   How many requests before a block?"
2. Hand it to the `pipeline-spike` subagent (Sonnet) — one agent per question.
   In the prompt: the spike number, the question, what's already known about the environment.
   Spikes are independent → launch them **in one message** so they run in parallel.
3. Collect the answers in `spikes/FINDINGS.md`: question → answer → facts (numbers, response
   codes, quotes) → consequence for the plan.

A spike that needs interaction (logging in by hand, a captcha, a key from the user) isn't
given to a subagent — do it in the main loop or ask the user.

If a spike refuted an assumption of the plan, say so plainly and go back to stage 2.
Production code never imports from `spikes/`; whatever's needed is rewritten at stage 5.

---

## 4. Contracts → contracts.md

You write shapes, not behaviour. Sections:

- **Data models** — as code (`@dataclass`, `TypedDict`, pydantic, TS interfaces).
  Every field: type, whether required, unit/format, an example value.
- **Public signatures** — the functions/classes through which modules talk to each other.
  The body is `...` or `raise NotImplementedError`.
- **Storage** — the database/file schema, keys, what counts as a record's identity (the dedup key).
- **Response schemas** — what exactly the external source returns and what we take from it.
- **Configuration** — every parameter, its type, default and source (file/env/CLI).
- **Secrets and environment** — which keys and access are needed, under which variable names,
  where they come from, which limits and pricing come with them. `contracts.md` gets **the
  variable name and how to obtain it**, never the value. Here you also record: `.env` is in
  `.gitignore`, `.env.example` has names without values, and what happens when a variable is
  missing (we fail with a clear error rather than silently running at half strength).
- **Errors** — the exception taxonomy and what the caller does with each.

The stage's check: from `contracts.md` you can write module stubs, and they fit together
by type without a single line of logic. If you can't, the contract is incomplete.

---

## 5. Vertical slice

The narrowest end-to-end path: one input → one processing step → one output.

- Hard-code everything unrelated to the path: one query, one page, one output format.
- No "while I'm here": no retries, no configs, no cache unless this path needs them.
- Real external dependencies, not mocks. The slice's point is to test the joints with reality.
- **Run it and show the output to the user.** The stage isn't closed until the slice has run live.

Once it works, immediately fill in the "How to run" section of `brief.md`: installing
dependencies, environment variables, the run command, the test command. Write exactly the
lines that just worked, not how it "should be". Future sessions and the stage-8 auditor read
this section — without it the audit turns into reading code instead of running it.

If the slice hits an architecture problem, go back to stage 4 and fix the contracts.
That's cheaper than hitting it at stage 7.

---

## 6. Tasks → tasks.md

Atomicity: one task = one meaningful commit. It's too big if the DoD contains "and" more
than twice.

```
- [ ] T-05 — Result pagination
      Files: src/olx/search.py
      Do: walk the pages up to N or until the results run out
      DoD: `pytest tests/test_search.py::test_pagination` is green; on a real
           query with >2 pages, >50 listings are returned
      Executor: subagent
      Depends on: T-03
```
The order in the file = the order of execution. Explicit dependencies only where the order
isn't obvious. Field names follow the artifact's language (in a Russian `tasks.md`:
`Файлы`, `Делаем`, `DoD`, `Исполнитель: субагент | главный цикл`, `Зависит от`).

You fill in the `Executor` field **here**, while you can see the whole picture: `subagent`
for routine work with a clear DoD, `main loop` for tasks that touch architecture or need
decisions. When in doubt — `main loop`.

---

## 7. Implementation

**One task per session.** This is the default pace; speed up only if the user asked for it.

The cycle: take the top unfinished task → do it → **check the DoD for real**
(run the test/command, not "looks right") → tick `[x]` → a 1–3 line report
→ stop and ask whether to continue.

Who does it — by the task's `Executor` field:
- `main loop` — you do it yourself.
- `subagent` — hand it to the `pipeline-task` subagent; in the prompt: the task number and
  the project path. When it returns, **check the DoD yourself**, with your own run. The
  subagent's report isn't proof. You tick `[x]`, not it.

Several consecutive `subagent` tasks with no dependencies on each other can be launched in
parallel in one message. Tasks that touch the same files — only one after another.

- The task grew → stop, rewrite it in `tasks.md` as several, then do them.
- Don't refactor neighbouring code "while you're at it". Noticed something — a new task in `tasks.md`.
- A decision that came up (chose a library, changed the approach) → a draft line for `decisions.md`.

### The three-attempt rule

If the DoD doesn't pass after three tries, **stop fixing**. From there you're not solving
the task but cycling through options, and each next attempt doesn't get any cheaper.

Three tries means three different approaches, not three repeats of one with small tweaks.
If it didn't work, diagnose which of three cases you're facing:

1. **Missing knowledge** about an external system or library → a spike, stage 3.
2. **The contract is wrong** — the task runs into a shape we invented → stage 4.
3. **The DoD is unverifiable** or describes the wrong thing → stage 6, reformulate.

Report to the user: what you tried (briefly, by approach), where each one got stuck,
which case you see and what you propose. The decision to go back to an earlier stage is the
user's — it's a gate.

---

## 8. Audit → audit.md

You do **not** run the audit yourself: you saw the code being written, and that blinds you.

Launch the `pipeline-auditor` subagent (Agent, `subagent_type: pipeline-auditor`; the model,
effort and instructions are pinned in its definition). In the prompt give only this
project's specifics: the root path, the run and test commands (the "How to run" section of
`brief.md`, filled in at stage 5), and what to pay particular attention to.
Don't retell what to check — that's in its definition.

If the "How to run" section is empty, don't launch the audit. Fill it in yourself first by
running the commands: without them the auditor checks the code by reading, and that's not
an audit.

Put the report in `docs/pipeline/audit.md`. Show the user **only what's critical**
and ask what to fix now and what goes to the backlog.

Treat the audit sceptically in both directions: the subagent may not have understood the
intent. Before fixing a "defect", check that it's a defect and not a contract it misread.

---

## 9. Feedback

Audit findings go back into the project:

- Bring `brief.md` in line with what was **built**, not what was intended: stack, structure,
  how to run, known limitations. A future session reads this file first, so it must be accurate.
- `decisions.md` is append-only. Only what can't be recovered from the code: why X instead
  of Y, why Z was dropped, what a spike showed. Absolute dates. Past entries aren't edited;
  if you changed your mind — a new entry linking to the old one.
- Remaining defects become tasks in `tasks.md`.
- `state.md`: the cycle is closed, what's next.

The next pass starts from stage 7, not stage 1.

---

## 10. Release → release.md

Only before a real release and only if the project is published somewhere.
An internal script "for yourself" skips this stage.

The stage-8 audit answered the question "does the code do what it promised". The release
answers a different one: **will it survive contact with the outside world** — other people's
hands, other people's input, other people's bots, a dropped network and a traffic bill.

**Order:**

1. Tell the user it's time to run the pre-launch checks and name the ones needed:
   - `/launch-security` — any application with a server, authentication or other people's data;
   - `/launch-web` — a public website or landing page.
   These skills **are invoked by the user**; you don't run them yourself: they're
   deliberately made so they don't trigger automatically.
2. The reports will appear in `docs/launch/`. Read them.
3. **Turn P1 findings into tasks in `tasks.md`** and fix them before shipping. P2/P3 go to
   the backlog, explicitly marked as deliberately postponed.
4. Items with the 🙋 status are questions for the user (real reviews, company details,
   key rotation, billing alerts). Collect them into one list and ask.
5. After shipping, check that it works **in production**, not locally: open it,
   submit the form, go through the main scenario. Record what exactly you checked.
6. Fill in `release.md`.

**The stage's red flag:** "let's ship and fix it later". Everything marked P1 is fixed
before, not after — after the launch this list gets a different priority and never gets fixed.
