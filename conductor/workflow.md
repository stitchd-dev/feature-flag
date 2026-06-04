# Project Workflow
<!-- Last refreshed: 2026-06-04 (post seqtest_20260603 — documented the separate live-ClickHouse explicit --test-target CI step gotcha that root-caused the red-CI fixed in cce4819) -->

## Guiding Principles

1. **The Plan is the Source of Truth:** All work must be tracked in `plan.md`
2. **The Tech Stack is Deliberate:** Changes to the tech stack must be documented in `tech-stack.md` *before* implementation
3. **Test-Driven Development:** Write unit tests before implementing functionality
4. **High Code Coverage:** Aim for ≥90% code coverage for all modules (CI enforces ≥90% via cargo-tarpaulin)
5. **User Experience First:** Every decision should prioritize user experience
6. **Non-Interactive & CI-Aware:** Prefer non-interactive commands. Use `CI=true` for watch-mode tools (tests, linters) to ensure single execution.

## Status Markers

- `[ ]` - Pending/New
- `[~]` - In Progress
- `[x]` - Completed
- `[!]` - Blocked (with reason)

### Blocker Format
When a task is blocked, use: `- [!] Task name [BLOCKED: reason]`

Example:
```markdown
- [!] Integrate payment API [BLOCKED: Waiting for API credentials from vendor]
```

## Task Workflow

All tasks follow a strict lifecycle:

### Standard Task Workflow

1. **Select Task:** Choose the next available task from `plan.md` in sequential order

2. **Mark In Progress:** Before beginning work, edit `plan.md` and change the task from `[ ]` to `[~]`

3. **Write Failing Tests (Red Phase):**
  - Create a new test file for the feature or bug fix.
  - Write one or more unit tests that clearly define the expected behavior and acceptance criteria for the task.
  - **CRITICAL:** Run the tests and confirm that they fail as expected. This is the "Red" phase of TDD. Do not proceed until you have failing tests.

4. **Implement to Pass Tests (Green Phase):**
  - Write the minimum amount of application code necessary to make the failing tests pass.
  - Run the test suite again and confirm that all tests now pass. This is the "Green" phase.

5. **Refactor (Optional but Recommended):**
  - With the safety of passing tests, refactor the implementation code and the test code to improve clarity, remove duplication, and enhance performance without changing the external behavior.
  - Rerun tests to ensure they still pass after refactoring.

6. **Verify Coverage:** Run coverage reports using the project's chosen tools. For example, in a Python project, this might look like:
   ```bash
   pytest --cov=app --cov-report=html
   ```
   Target: ≥90% coverage for new code. Run `cargo tarpaulin -p <crate_name>` to check locally.

7. **Document Deviations:** If implementation differs from tech stack:
  - **STOP** implementation
  - Update `tech-stack.md` with new design
  - Add dated note explaining the change
  - Resume implementation

8. **Commit Code Changes:**
  - Stage all code changes related to the task.
  - Propose a clear, concise commit message e.g, `feat(ui): Create basic HTML structure for calculator`.
  - Perform the commit.

9. **Attach Task Summary with Git Notes:**
  - **Step 9.1: Get Commit Hash:** Obtain the hash of the *just-completed commit* (`git log -1 --format="%H"`).
  - **Step 9.2: Draft Note Content:** Create a detailed summary for the completed task. This should include the task name, a summary of changes, a list of all created/modified files, and the core "why" for the change.
  - **Step 9.3: Attach Note:** Use the `git notes` command to attach the summary to the commit.
    ```bash
    # The note content from the previous step is passed via the -m flag.
    git notes add -m "<note content>" <commit_hash>
    ```

10. **Get and Record Task Commit SHA:**
  - **Step 10.1: Update Plan:** Read `plan.md`, find the line for the completed task, update its status from `[~]` to `[x]`, and append the first 7 characters of the *just-completed commit's* commit hash.
  - **Step 10.2: Write Plan:** Write the updated content back to `plan.md`.

11. **Commit Plan Update:**
  - **Action:** Stage the modified `plan.md` file.
  - **Action:** Commit this change with a descriptive message (e.g., `conductor(plan): Mark task 'Create user model' as complete`).

### Phase Completion Verification and Checkpointing Protocol

**Trigger:** This protocol is executed immediately after a task is completed that also concludes a phase in `plan.md`.

1.  **Announce Protocol Start:** Inform the user that the phase is complete and the verification and checkpointing protocol has begun.

2.  **Ensure Test Coverage for Phase Changes:**
  -   **Step 2.1: Determine Phase Scope:** To identify the files changed in this phase, you must first find the starting point. Read `plan.md` to find the Git commit SHA of the *previous* phase's checkpoint. If no previous checkpoint exists, the scope is all changes since the first commit.
  -   **Step 2.2: List Changed Files:** Execute `git diff --name-only <previous_checkpoint_sha> HEAD` to get a precise list of all files modified during this phase.
  -   **Step 2.3: Verify and Create Tests:** For each file in the list:
    -   **CRITICAL:** First, check its extension. Exclude non-code files (e.g., `.json`, `.md`, `.yaml`).
    -   For each remaining code file, verify a corresponding test file exists.
    -   If a test file is missing, you **must** create one. Before writing the test, **first, analyze other test files in the repository to determine the correct naming convention and testing style.** The new tests **must** validate the functionality described in this phase's tasks (`plan.md`).

3.  **Execute Automated Tests with Proactive Debugging:**
  -   Before execution, you **must** announce the exact shell command you will use to run the tests.
  -   **Example Announcement:** "I will now run the automated test suite to verify the phase. **Command:** `CI=true npm test`"
  -   Execute the announced command.
  -   If tests fail, you **must** inform the user and begin debugging. You may attempt to propose a fix a **maximum of two times**. If the tests still fail after your second proposed fix, you **must stop**, report the persistent failure, and ask the user for guidance.

4.  **Propose a Detailed, Actionable Manual Verification Plan:**
  -   **CRITICAL:** To generate the plan, first analyze `product.md`, `product-guidelines.md`, and `plan.md` to determine the user-facing goals of the completed phase.
  -   You **must** generate a step-by-step plan that walks the user through the verification process, including any necessary commands and specific, expected outcomes.
  -   The plan you present to the user **must** follow this format:

      **For a Frontend Change:**
      ```
      The automated tests have passed. For manual verification, please follow these steps:

      **Manual Verification Steps:**
      1.  **Start the development server with the command:** `npm run dev`
      2.  **Open your browser to:** `http://localhost:3000`
      3.  **Confirm that you see:** The new user profile page, with the user's name and email displayed correctly.
      ```

      **For a Backend Change:**
      ```
      The automated tests have passed. For manual verification, please follow these steps:

      **Manual Verification Steps:**
      1.  **Ensure the server is running.**
      2.  **Execute the following command in your terminal:** `curl -X POST http://localhost:8080/api/v1/users -d '{"name": "test"}'`
      3.  **Confirm that you receive:** A JSON response with a status of `201 Created`.
      ```

5.  **Await Explicit User Feedback:**
  -   After presenting the detailed plan, ask the user for confirmation: "**Does this meet your expectations? Please confirm with yes or provide feedback on what needs to be changed.**"
  -   **PAUSE** and await the user's response. Do not proceed without an explicit yes or confirmation.

6.  **Create Checkpoint Commit:**
  -   Stage all changes. If no changes occurred in this step, proceed with an empty commit.
  -   Perform the commit with a clear and concise message (e.g., `conductor(checkpoint): Checkpoint end of Phase X`).

7.  **Attach Auditable Verification Report using Git Notes:**
  -   **Step 8.1: Draft Note Content:** Create a detailed verification report including the automated test command, the manual verification steps, and the user's confirmation.
  -   **Step 8.2: Attach Note:** Use the `git notes` command and the full commit hash from the previous step to attach the full report to the checkpoint commit.

8.  **Get and Record Phase Checkpoint SHA:**
  -   **Step 7.1: Get Commit Hash:** Obtain the hash of the *just-created checkpoint commit* (`git log -1 --format="%H"`).
  -   **Step 7.2: Update Plan:** Read `plan.md`, find the heading for the completed phase, and append the first 7 characters of the commit hash in the format `[checkpoint: <sha>]`.
  -   **Step 7.3: Write Plan:** Write the updated content back to `plan.md`.

9. **Commit Plan Update:**
  - **Action:** Stage the modified `plan.md` file.
  - **Action:** Commit this change with a descriptive message following the format `conductor(plan): Mark phase '<PHASE NAME>' as complete`.

10.  **Announce Completion:** Inform the user that the phase is complete and the checkpoint has been created, with the detailed verification report attached as a git note.

### Quality Gates

Before marking any task complete, verify:

- [ ] All tests pass
- [ ] Code coverage meets requirements (≥90%)
- [ ] Code follows project's code style guidelines (as defined in `code_styleguides/`)
- [ ] All public functions/methods are documented (e.g., docstrings, JSDoc, GoDoc)
- [ ] Type safety is enforced (e.g., type hints, TypeScript types, Go types)
- [ ] No linting or static analysis errors (using the project's configured tools)
- [ ] Works correctly on mobile (if applicable)
- [ ] Documentation updated if needed
- [ ] No security vulnerabilities introduced

## Development Commands

### Setup
```bash
# Install sqlx-cli (compile-time query checking)
cargo install sqlx-cli --no-default-features --features rustls,postgres

# Start infrastructure (Postgres + ClickHouse + ScyllaDB — all three required)
docker compose up postgres clickhouse scylladb -d --wait

# Run DB migrations
cargo sqlx migrate run --source crates/stitchd-db/migrations

# sqlx-cli requires plain DATABASE_URL (not STITCHD_DATABASE_URL)
# Always alias before running sqlx commands:
# export DATABASE_URL="$STITCHD_DATABASE_URL"
```

### Daily Development
```bash
# Run all tests (requires running postgres + clickhouse)
cargo test --workspace

# Run tests for a specific crate
cargo test -p stitchd-auth-service

# Format code
cargo fmt --all

# Lint (mirrors CI — all warnings are errors)
cargo clippy --workspace --all-targets -- -D warnings

# Build docs site (gRPC pages → OpenAPI → env-vars → crate-READMEs →
# Quickstart → mdbook → rustdoc → internal-link check)
cargo run --manifest-path crates/xtask/Cargo.toml -- docs

# `cargo xtask docs` is idempotent — a second run produces zero diff.
# CI enforces this via `cargo xtask docs && git diff --exit-code`.
# If you hand-edit a generator-owned file (anything under docs/src/grpc/
# except README.md, docs/src/deployment/env-vars.md, docs/src/sdk/quickstart.md,
# docs/src/api/openapi.json, or crates/*/README.md), CI will fail until
# the edit is moved into the source-of-truth (proto, //! preamble, env-var
# declaration, lib.rs Quickstart section).
```

### Admin UI (Frontend — `admin/` directory)
```bash
# Install dependencies
cd admin && npm install

# Start dev server (proxies /api → gateway on :8080)
npm run dev

# Type-check (NEVER use npx tsc — resolves to stray tsc 2.0.x package)
node_modules/.bin/tsc --noEmit -p tsconfig.app.json

# Lint
npm run lint

# Production build
npm run build
```

### Before Committing
```bash
# Full pre-commit check (format + lint + tests)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# After adding OR removing sqlx::query!/sqlx::query_scalar! macros — regenerate
# the offline cache. Use the SAME flags the CI `sqlx-check` job verifies with
# (`--all-targets --features stitchd-sdk-rust/test-util`), NOT a narrower
# `-- --tests`. Preparing with only `--tests` can leave queries that compile
# under `--all-targets`/the test-util feature uncached, so CI's
# `cargo sqlx prepare --workspace --check -- --all-targets --features
# stitchd-sdk-rust/test-util` then fails with `no cached data for this query`.
# (Conversely, dropping a query — e.g. PROP-001 removing the frozen column —
# prunes its cache entry; commit the .sqlx/ deletion.)
SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets --features stitchd-sdk-rust/test-util

# After modifying a //! preamble, env-var, or .proto — regenerate doc
# artifacts and confirm zero drift (this is what CI runs):
cargo run --manifest-path crates/xtask/Cargo.toml -- docs
git diff --exit-code
```

### CI Environment Notes
- `SQLX_OFFLINE=true` in CI — all `sqlx::query!` macros use the `.sqlx/` offline cache
- `sqlx-check` job runs `cargo sqlx prepare --workspace --check -- --all-targets --features stitchd-sdk-rust/test-util` to verify cache completeness across all targets and tests.
- `docs-build` job is gated on both backend `coverage` (cargo-llvm-cov) and vitest `admin-frontend` jobs to prevent regressions from slipping through.
- CI only starts `postgres` and `clickhouse` containers; the six microservice containers are exercised by E2E Step CI workflows in `tests/e2e/`
- Coverage threshold: ≥90% per crate (cargo-tarpaulin/cargo-llvm-cov, uploaded to Codecov per crate flag)
- `contract-check` job verifies the gateway covers the pre-decomposition OpenAPI surface (`scripts/check_openapi_contract.py`)
- **Live-ClickHouse stats tests run in a SEPARATE Coverage-job step with an EXPLICIT `--test` list — keep it in sync.** `cargo llvm-cov` (the Coverage job's main pass) does NOT run `#[ignore]`d tests, so the self-seeding live-CH integration tests in `stitchd-stats-service` (they call `event_writer::migrations::run` to build their own tables) run in a dedicated **"Live-ClickHouse integration tests (stats-service)"** step that names each `--test` target by filename and passes `-- --ignored`. **When you add, rename, or remove a self-seeding `tests/*.rs` file in `stitchd-stats-service`, update that step's `--test` list in `.github/workflows/ci.yml`** — a stale target makes cargo exit 101 (`no test target named X`), turning the Coverage job (and all of CI) red on the *next* push, **invisible to local `cargo test --workspace`** (which never runs that step). Current set: `aggregation_query, ratio_query, funnel_query, preview_query, interaction_compute, compute_pass, cuped_compute, percentile_significance`. (A stale `interaction_query` left by the N-way rename reddened CI from the nway merge through the seqtest merge until `cce4819` rebuilt the full 8-target list.)

## Testing Requirements

### Unit Testing
- Every module must have corresponding tests.
- Use appropriate test setup/teardown mechanisms (e.g., fixtures, beforeEach/afterEach).
- Mock external dependencies.
- Test both success and failure cases.

### Integration Testing
- Test complete user flows
- Verify database transactions
- Test authentication and authorization
- Check form submissions

### Mobile Testing
- Test on actual iPhone when possible
- Use Safari developer tools
- Test touch interactions
- Verify responsive layouts
- Check performance on 3G/4G

## Code Review Process

### Self-Review Checklist
Before requesting review:

1. **Functionality**
  - Feature works as specified
  - Edge cases handled
  - Error messages are user-friendly

2. **Code Quality**
  - Follows style guide
  - DRY principle applied
  - Clear variable/function names
  - Appropriate comments

3. **Testing**
  - Unit tests comprehensive
  - Integration tests pass
  - Coverage adequate (>95%)

4. **Security**
  - No hardcoded secrets
  - Input validation present
  - SQL injection prevented
  - XSS protection in place

5. **Performance**
  - Database queries optimized
  - Images optimized
  - Caching implemented where needed

6. **Mobile Experience**
  - Touch targets adequate (44x44px)
  - Text readable without zooming
  - Performance acceptable on mobile
  - Interactions feel native

## Commit Guidelines

### Message Format
```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

### Types
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Formatting, missing semicolons, etc.
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `test`: Adding missing tests
- `chore`: Maintenance tasks

### Examples
```bash
git commit -m "feat(auth): Add remember me functionality"
git commit -m "fix(posts): Correct excerpt generation for short posts"
git commit -m "test(comments): Add tests for emoji reaction limits"
git commit -m "style(mobile): Improve button touch targets"
```

## Definition of Done

A task is complete when:

1. All code implemented to specification
2. Unit tests written and passing
3. Code coverage meets project requirements
4. Documentation complete (if applicable)
5. Code passes all configured linting and static analysis checks
6. Works beautifully on mobile (if applicable)
7. Implementation notes added to `plan.md`
8. Changes committed with proper message
9. Git note with task summary attached to the commit

## Emergency Procedures

### Critical Bug in Production
1. Create hotfix branch from main
2. Write failing test for bug
3. Implement minimal fix
4. Test thoroughly including mobile
5. Deploy immediately
6. Document in plan.md

### Data Loss
1. Stop all write operations
2. Restore from latest backup
3. Verify data integrity
4. Document incident
5. Update backup procedures

### Security Breach
1. Rotate all secrets immediately
2. Review access logs
3. Patch vulnerability
4. Notify affected users (if any)
5. Document and update security procedures

## Deployment Workflow

### Pre-Deployment Checklist
- [ ] All tests passing
- [ ] Coverage ≥90%
- [ ] No linting errors
- [ ] Mobile testing complete
- [ ] Environment variables configured
- [ ] Database migrations ready
- [ ] Backup created

### Git Push Policy
**IMPORTANT:** Conductor commits locally but **NEVER pushes automatically**.
- All commits remain local until the user explicitly pushes
- Users decide when and how to push to remote repositories
- This allows for commit squashing, rebase, or other git workflows before pushing

### Deployment Steps
1. Merge feature branch to main
2. Tag release with version
3. Push to deployment service
4. Run database migrations
5. Verify deployment
6. Test critical paths
7. Monitor for errors

### Post-Deployment
1. Monitor analytics
2. Check error logs
3. Gather user feedback
4. Plan next iteration

## Parallel Sub-Agent Workflow

For large tracks (many tasks per phase, strong parallelism), use a **parallel worker-wave model**. This pattern was first used at scale in `boundaries_20260518` (7 phases, 79 commits, 19 sub-agent workers).

### Core model

1. **Orchestrator agent** manages the track — reads the plan, assigns tasks to worker agents, aggregates results, merges branches.
2. **Worker agents** operate in isolated git worktrees (`.worktrees/<track_id>_w<N>/`) created with `git worktree add`. Each worker handles one task: write code, run tests, commit, close beads task.
3. Tasks are batched into **waves** — groups of independently executable tasks that share no intra-wave compile-time dependencies. Workers in a wave run concurrently; the orchestrator waits for all to finish before starting the next wave.

### Worktree discipline

- Each worktree gets its own `target/` directory — first build is slow (5–10 min for backend; 1–3 min for doc-only), but isolation is complete.
- Always run `cargo test/clippy` from inside the worktree (`cd .worktrees/<track_id>/` or `cargo -C <path>`). Running from the main repo root silently compiles the main branch.
- After all workers close, the orchestrator merges worker branches into the track branch with `git merge --no-ff` and deletes the worker branches with `git branch -D` (not `-d` — diverged branches require force delete).

### Task lifecycle (per worker)

1. Pick up a beads task (`bd update --status in_progress`).
2. Implement, test per-crate (`cargo test -p <crate>`), commit.
3. Close the beads task with **`bd close --no-auto`** — this prevents beads from cascading into the next phase's tasks before the orchestrator has verified the milestone.
4. Report back to orchestrator with commit SHA and any discovered out-of-scope issues.

### `bd close --no-auto` is mandatory for parallel waves

Using `bd close --continue` (or the default auto-advance) with multiple concurrent workers causes beads to claim tasks from subsequent phases into `in_progress`, even when those phases are blocked behind a milestone dependency. This requires manual reset (`bd update ... --status open --assignee ""`). Always use `--no-auto`; let the orchestrator control wave advancement.

### Fix gaps as discovered

If a worker finds a genuine bug or drift in the current task's scope, **fix it inline and note it in the report-back**. Do not defer clearly in-scope fixes. For pre-existing issues clearly outside the current task scope, file a new beads bug with `bd create --priority 2` and reference it — do not fix inline.

### Worker beads-close audit

After every wave, the orchestrator verifies that every worker's beads task state matches its commit. Workers that hit an agent-runtime cutoff may leave tasks `in_progress` with no commit. Close these manually with `bd note "commit: <sha>"` + `bd close --no-auto`.

### Parallel trait reconciliation

When two workers define overlapping traits for the same domain (e.g. a handler-side and a repo-side worker both write a "canonical" trait), the **repo-side worker owns the trait** — it controls storage semantics. Handler-side types and method signatures are aligned to the repo-side definition during merge integration. Use the proto schema as the natural alignment point for cross-service type disagreements (add shared fields to the message; renumber if needed).

### File-ownership boundary in worker prompts

Before spawning parallel workers, write the **file-ownership table** explicitly into each worker prompt. List the files THIS worker owns + the files the SIBLING worker(s) own — with a hard "NEVER edit files outside the worktree, especially X/Y/Z" rule. The 11-phase `experimentation_full_20260521` track ran 14 parallel workers (Phases 2+3, 5+6, 9+10, plus three bug-fix workers in parallel) with **zero merge conflicts on source files** — only `learnings.md` ever conflicted (both workers append). The cost of writing the boundary table is one minute; the savings on merge are large.

When a worker's "natural" scope inevitably touches a sibling's file (e.g. a wider integration commit), make that ONE file an explicit shared seam in BOTH prompts (e.g. "you both edit `service.rs::new(...)` — keep your additions small + adjacent"). Phase 8's `FlagServiceImpl::new(...)` was such a seam and survived.

### Beads close gotcha (`--no-auto` doesn't reliably persist)

Workers should close their tasks with plain `bd close <id>` and `--force` when needed (phantom dep on a still-open sibling phase). The conductor-implement protocol's `--no-auto` directive is unreliable in current Beads — closures may silently re-open. Documented in `conductor/patterns.md` for the "Experimentation Patterns" section.

## Continuous Improvement

- Review workflow weekly
- Update based on pain points
- Document lessons learned
- Optimize for user happiness
- Keep things simple and maintainable
