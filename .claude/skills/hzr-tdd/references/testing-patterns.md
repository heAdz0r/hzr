# HZR testing patterns

## Pure behavior

Keep focused unit tests beside the module when the behavior is a pure function,
parser, validator, or typed state transition. Assert the public behavior and its
important boundary cases, not private implementation steps.

## CLI behavior

Use a process-level integration test with `env!("CARGO_BIN_EXE_hzr")` when exit
status, stdout, stderr, argument parsing, or environment isolation is part of the
contract. Run it against an isolated temporary HOME and configuration whenever
the command could otherwise touch user state.

## Daemon and protocol behavior

Test typed request and response structures directly. Do not parse human CLI
output when JSON is available. For lifecycle behavior, verify ownership,
idempotency, shutdown, and error propagation explicitly.

## Fork-core behavior

Add inherited-engine regression tests under `fork-core/rtk` only when the behavior
belongs to command execution or output filtering. Preserve upstream parity and run
the complete fork gate after intentionally refreshing the current-engine identity.

## Valid RED evidence

- an assertion fails on the behavior being added or fixed;
- the new test does not compile because the deliberately designed API is absent;
- a process test receives the old exit status or output instead of the required contract.

## Invalid RED evidence

- the test cannot start because a dependency is missing;
- another dirty-worktree change breaks compilation first;
- the daemon or network is unavailable for behavior that should be tested locally;
- the test is flaky or fails for a reason unrelated to its assertion.
