<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

## Project Agents

Specialised Claude Code sub-agents are defined in `.claude/agents/`. Invoke them by name (e.g. `use the developer agent`).

### Development workflow

| Agent | When to use |
| --- | --- |
| **developer** | Implement a feature, fix a bug, or make code changes. Follows project conventions, does not over-engineer. |
| **architect** | Root cause diagnosis and concrete fix design — after investigation findings are available. |
| **investigator** | Trace unexpected behaviour, audit a data flow, or gather evidence before a fix is written. Does not propose fixes. |
| **contrarian** | Stress-test a diagnosis or proposed fix before acting on it. Catches confirmation bias and coverage gaps. |

### Code quality

| Agent | When to use |
| --- | --- |
| **refactoring-engineer** | After a feature is implemented and tests pass — improve modularity, readability, and structure without changing behaviour. |
| **simplifier** | After a feature is implemented and tests pass — remove unnecessary complexity, duplication, and dead code. |
| **logging-implementer** | Backfill structured tracing on a new feature, or audit whether an existing feature's logging is complete. |

### Testing

| Agent | When to use |
| --- | --- |
| **test-developer** | Write tests for new or existing functionality. Thinks adversarially — happy paths, edge cases, failure modes. |
| **test-runner** | Run the test suite after code changes and report exactly what is broken and why. Does not fix code. |
| **stress-tester** | Design and implement stress, load, and throughput tests for concurrency correctness or resource exhaustion. |

### Planning and documentation

| Agent | When to use |
| --- | --- |
| **pm** | Create an OpenSpec change proposal, write documentation, or produce a task checklist. |

### Packaging

| Agent | When to use |
| --- | --- |
| **dev-packager** | Build a local dev package (debug + `--features tray`). Produces a `file://` Homebrew tarball and an unsigned `.pkg`. Invoked by `/privacyclaw:package`. |
| **prod-packager** | Build a production release (universal arm64+x86_64, `--features tray`). Produces signed/notarized `.pkg`, per-arch Homebrew tarballs, and pushes the updated tap formula to the tap repo. Invoked by `/privacyclaw:package`. |

## Feature development workflow

Use `/privacyclaw:implement "description"` to run the full 4-phase workflow:

1. **Design** — Architect, Investigator, Contrarian produce and challenge a Design Document.
2. **Planning** — PM and Architect build an OpenSpec task list; Investigator and Contrarian review it.
3. **Development** — Developer implements each task; Refactoring Engineer, Simplifier, and Logging Implementer polish it; Contrarian approves.
4. **Testing** — Test Developer and Stress Tester write tests; Test Runner gates completion.

Skills live in `.claude/skills/privacyclaw/`. Each phase is also independently
invocable: `/privacyclaw:design`, `/privacyclaw:plan`, `/privacyclaw:develop`,
`/privacyclaw:test`. Each phase runs in an isolated `context: fork` to prevent
context exhaustion across the full workflow.

## Packaging workflow

Use `/privacyclaw:package` to build and release a new version. The skill asks
three questions interactively (branch, target env, version bump type), then:

1. Checks the working tree is clean (prompts before committing if not)
2. Runs `cargo test` — aborts on any failure
3. Bumps the version in `Cargo.toml` and tap files
4. Invokes `dev-packager`, `prod-packager`, or both in parallel
5. Verifies all artifacts on disk before committing anything
6. Commits version bump, creates and pushes git tag `v<VERSION>`
7. Uploads `.pkg` and tarballs to a GitHub Release (prod only)
8. Pulls local to match the new tag
