## Skills Usage Order

Use commands in this exact order:

1. `/requirements`
2. `/prepare`
3. `/planning`
4. `/implement`
5. `/commit`
6. `/summarize`

## How to Use Each Skill (with examples)

### 1) `/requirements`
- Purpose: validate feasibility, clarify scope, and create requirements.
- Input: user request.
- Output: `.workflow/requirements/{task-name}.md`.
- Example: `/requirements Analyze this request and create .workflow/requirements/audio-transcription-badges.md.`

### 2) `/prepare`
- Purpose: create GitHub issue and implementation branch from approved requirements.
- Input: `.workflow/requirements/{task-name}.md`.
- Output: issue content/status and branch content/status.
- Example: `/prepare .workflow/requirements/audio-transcription-badges.md`

### 3) `/planning`
- Purpose: create implementation plan with traceability, tests, and risk coverage.
- Input: `.workflow/requirements/{task-name}.md`.
- Output: `.workflow/plans/{task-name}.md`.
- Example: `/planning .workflow/requirements/audio-transcription-badges.md`

### 4) `/implement`
- Purpose: execute plan changes, update tests, and validate regressions.
- Input: `.workflow/plans/{task-name}.md` and `.workflow/requirements/{task-name}.md`, or inline context.
- Output: implemented changes, tests, validation evidence, self-review result.
- Includes a **self-review cycle** (max 3 attempts): after implementation, tests are run and failures are fixed iteratively. If all tests pass, the skill finalizes. If failures persist after 3 attempts, implementation stops and a failure report with root cause analysis and adjustment plan is returned to the user.
- Example (local files): `/implement audio-transcription-badges`
- Example (inline context): see [Flexible Usage](#flexible-usage) below

### 5) `/commit`
- Purpose: create focused Conventional Commits according to `CONTRIBUTING.md`.
- Input: current working tree after implementation/validation.
- Output: scoped commits.
- Example: `/commit Create focused Conventional Commits for all pending changes.`

### 6) `/summarize`
- Purpose: compare delivery vs requirements and generate PR-ready summary. Can also run standalone from git history.
- Input: requirements + plan + branch state, external reference URL, or empty (git-only).
- Output: `.workflow/summaries/{task-name}.md` or inline summary.
- Example (local files): `/summarize audio-transcription-badges`
- Example (git-only): `/summarize`
- Example (external ref): `/summarize https://dev.azure.com/org/project/_workitems/edit/1234`

### 7) `/document` (standalone)
- Purpose: analyze the codebase and generate a project knowledge base.
- Input: codebase + optional user context.
- Output: `architecture.md` (tech stack, dependencies, patterns, auth, roles) and `requirements.md` (functional, non-functional, business rules with file references).
- Can be used at any point — independent of the pipeline.
- When these files exist, `/planning` and `/implement` automatically use them as constraints.
- Example: `/document`
- Example: `/document Focus on the authentication and authorization layers`

## Flexible Usage

Both `/implement` and `/summarize` support flexible input modes, allowing you to skip the full pipeline when requirements already exist externally.

### `/implement` with Inline Context

When you already have requirements defined elsewhere (Azure DevOps, GitHub Projects, etc.), you can pass them directly:

```
/implement ## Inline Context

### Scope
- Add retry logic to the payment gateway client
- Out of scope: billing UI changes

### Acceptance Criteria
- Failed requests are retried up to 3 times with exponential backoff
- Circuit breaker opens after 5 consecutive failures
- All retry attempts are logged with correlation ID

### Implementation Approach
- Wrap existing `PaymentClient.send()` with retry decorator
- Add circuit breaker state machine in `lib/resilience/`
- Unit tests for retry counts and backoff timing
```

**Minimum required sections for inline context:**

| Section | Purpose |
|---------|---------|
| **Scope** | What changes, what is out of scope |
| **Acceptance Criteria** | Testable conditions for "done" |
| **Implementation Approach** | How to structure, phase, and test changes |

Optional: **External Reference** (URL to ticket), **Regression Concerns** (known risk areas).

### `/summarize` Independent Modes

| Mode | Trigger | Deviation Analysis |
|------|---------|-------------------|
| **Git-only** | No arguments or branch/range only | No — summary from git history only |
| **External reference** | URL or pasted requirements | Yes — compares delivery against reference |
| **Local files** | Task slug | Yes — compares against `.workflow/requirements/` |
