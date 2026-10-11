# Cross-application compatibility validation

`cargo xtask compat` tests proposed changes in `craft-libs` against consuming Craft applications before those changes are adopted across the suite. It runs each application's existing test suite against both its committed dependencies and the proposed library revision, without modifying the application's source repository, committed dependency pins, or lockfiles.

## How it works

The runner operates in two distinct modes for each consuming application:

1. **Baseline**: Tests the application using its committed dependencies and lockfile. This establishes whether any test failures already exist on the consumer's current branch.
2. **Candidate**: Tests the application against a specified `craft-libs` commit, branch, or working tree using Cargo's native patch configuration (`--config 'patch."https://github.com/storytold/craft-libs".craft-ui.path="<candidate-path>"'`).

To protect consumer repositories from accidental modification or lockfile changes, candidate evaluation runs in an ephemeral Git worktree created under `target/compat/worktrees/`. A drop guard ensures that temporary worktrees are removed upon completion, interruption (`SIGINT`/`SIGTERM`), or failure.

Before executing any candidate tests, the runner runs `cargo tree -i craft-ui` in the worktree to verify that Cargo actually resolved the candidate path rather than the pinned Git revision. If the resolved source does not match the candidate, validation aborts immediately.

## Results classification

Every consumer check is evaluated across both runs:

| Baseline | Candidate | Classification | Description | Exit status |
| :---: | :---: | :--- | :--- | :---: |
| Pass | Pass | `COMPATIBLE` | Candidate revision preserves application compatibility | 0 |
| Pass | Fail | `CANDIDATE_REGRESSION` | Candidate revision introduced a failure | 1 |
| Fail | Fail | `BASELINE_FAILURE` | Pre-existing failure on consumer's current branch | 0 |
| Fail | Pass | `FIXED_IN_CANDIDATE` | Candidate resolved an existing issue | 0 |
| — | — | `SKIPPED` | Consumer directory not found or check skipped | 0 |

A run exits with a non-zero status if any candidate regression occurs. Baseline failures are reported prominently in the summary table but do not block library validation because the failure pre-existed in the consumer.

## Running

```sh
# Test both PhotoCraft and VectorCraft against current working tree (HEAD)
cargo xtask compat

# Test against a specific craft-libs commit
cargo xtask compat --revision 5c874d51

# Test specific consumers
cargo xtask compat --consumers photocraft,vectorcraft

# Validate worktree and dependency resolution without running full test suites
cargo xtask compat --dry-run

# Output machine-readable JSON
cargo xtask compat --json
```

Detailed test outputs and compiler logs are saved to `target/compat/logs/<consumer>-<mode>.log`. A structured summary is written to `target/compat/report.json`.
