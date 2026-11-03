# Artifact templates

Copy the structure, but don't carry empty sections: if there's nothing to write in a section,
delete it rather than leaving "TBD". The exception is `state.md`, which is always complete.

The templates are in English; write the artifacts in the user's language (code and
identifiers stay in English).

---

## state.md

```markdown
# Pipeline state

Project: <name>
Updated: <YYYY-MM-DD>

Current stage: **7 — implementation**
Gates: passed up to and including stage 6 (confirmed by the user on 2026-07-22)

Next action: T-05 "Result pagination"

## Stage history
- 0 context load — 2026-07-22
- 1 idea — 2026-07-22
- 2 plan + criteria — 2026-07-22
- 3 risk spikes — 2026-07-22 (spike 02 changed the plan: see decisions.md)
- ...
```

---

## brief.md

```markdown
# Project Brief: <name>

## Gist
3–5 sentences: what we build, for whom, what pain we remove, what counts as success.

## User and scenario
Who runs it, how often, from where (CLI / a schedule / other people).

## Scope
**In scope:** ...
**Out of scope (non-goals):** ...

## Stack and environment
Language and version, key libraries (with versions and why them), OS specifics.

## External dependencies
Data source / API: address, authorisation, licence, ToS restrictions, what's known
about protection and limits.

## Constraints and risks
A list. Each risk says what closes it: a spike, the design, or "accepted as is".

## How to run
Installing dependencies, environment variables, the run command, the test command.
Filled in at stage 5 with the commands that actually worked, and refined at stage 9.
An empty section blocks the audit: the auditor has nothing to check with.

## Glossary
Domain terms, if they aren't self-evident.
```

---

## plan.md

```markdown
# Plan

## Steps
1. **<Step>** — what we do, why. Done = AC-1, AC-2.
2. ...

## Acceptance criteria
AC-1: Given — <initial state/input>.
      When — <the action, literally a command>.
      Then — <an observable result, verifiable without an opinion>.

AC-2: ...

## Non-goals
- ...

## Open questions
- <question> → closed by spike NN / the user's decision.
```

---

## spikes/FINDINGS.md

```markdown
# Spike results

## S-01 — <the question in one line>
**Script:** `spikes/01-name.py`
**Answer:** yes / no / partly
**Facts:** response codes, timings, volumes, quotes from the output.
**Consequence for the plan:** what we change in plan.md / contracts.md.
```

---

## contracts.md

```markdown
# Contracts

## Data models
```python
@dataclass(frozen=True)
class Listing:
    id: str            # OLX listing id, "1234567890"
    title: str
    price: int | None  # in the `currency` currency; None = "negotiable"
    currency: str      # ISO-4217, "UAH"
    url: str
    posted_at: datetime  # UTC, aware
```

## Public signatures
```python
def search(query: str, filters: Filters, *, max_pages: int = 3) -> list[Listing]: ...
```

## Storage
File/database, schema, the identity key (dedup key) and why that one.

## Source response schemas
What actually arrives and which fields we take. With an example.

## Configuration
| Parameter | Type | Default | Source | Meaning |
|---|---|---|---|---|

## Secrets and access
| Variable | What it is | Where to get it | When it's missing |
|---|---|---|---|
| `OLX_API_KEY` | key for the listings API | the account dashboard, "For developers" section | fail at startup |

Values are never written here. `.env` is in `.gitignore`, `.env.example` has names
without values.

## Errors
| Exception | When | What the caller does |
|---|---|---|
```

---

## tasks.md

```markdown
# Tasks

- [x] T-01 — <title>
      Files: ...
      Do: ...
      DoD: <a verifiable criterion: a command, a test, an observable result>
      Executor: subagent | main loop
      Depends on: —

- [ ] T-02 — ...
```

---

## decisions.md

```markdown
# Decision log

Append-only. Past entries are not edited.

## 2026-07-22 — <the decision in one line>
**Context:** what forced a choice.
**Decision:** what we chose.
**Alternatives:** what we considered and why we rejected it.
**Consequences:** what we pay with, what's now impossible/possible.
**Source:** spike S-02 / the audit / a discussion with the user.
```

---

## audit.md

Written by the subagent. Structure:

```markdown
# Audit <date>

## Critical
- `src/olx/search.py:88` — <fact>. AC-3 is not met: <how it was checked>.

## Worth fixing
- ...

## Observations
- ...

## Acceptance criteria — summary
| AC | Status | How it was checked |
|---|---|---|
```

---

## release.md

```markdown
# Release <YYYY-MM-DD>

What was shipped: <version / commit>
Where: <hosting, domain, environment>

## Pre-launch checks
| Check | Report | P1s closed |
|---|---|---|
| /launch-security | `docs/launch/security-2026-08-13.md` | yes |
| /launch-web | `docs/launch/web-2026-08-13.md` | yes, except WEB-17 (see below) |

## Checked after release
What was opened and run **in production**, not locally. As concrete actions:
"sent a request from a phone — the email arrived in 8 seconds".

## Deliberately postponed
Item, priority, why it was postponed, when we'll come back to it.

## Rollback
How to restore the previous version, how long it takes, what's lost.

## Access
Who has access to the hosting, domain, database, email, analytics, payments.
Where the secrets live. What to do if a key leaks.
```
