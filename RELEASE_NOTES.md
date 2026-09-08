# HZR 0.9.1

A hotfix for the feature 0.9.0 shipped, plus a dashboard that explains itself:
what is wrong, which commands ran, what they cost, and what HZR saved.

## `hzr agents component install` works from an installed HZR

`install.sh` links `~/.local/bin/hzr` to the release binary and tells you to put
that directory on PATH. Through that link, `current_exe()` on macOS reports the
link rather than its target, and the bundle root derived from it was two
directories short of the packaged observer patch. Every installed user who ran
the command outside a source checkout was told to run it from one. The
executable is resolved before the root is derived — the same correction the
visualizer asset lookup has needed since 0.6 — and the command now finds its
patch from any directory.

## A warning says what is wrong and how to fix it

A workspace card marked `Warning` used to show a coloured chip, four grey dots
and a placeholder command. It now shows, in the card:

- **what is wrong** — which index artifacts are absent, or that the index
  directory or the workspace itself is missing;
- **how to fix it** — a real `hzr index init --workspace ~/Programming/compass`
  with your path already in it, copyable in one click;
- when nothing can fix it from here — a directory that no longer exists — an
  explanation and no command, because a command that would fail is worse than
  none.

The index artifact lights say what each artifact is for (config, semantic
search, code structure, repository map) and whether it is present.

## `Standby` over a working control plane is gone

Two bugs made a healthy daemon look idle. The posture chip matched the selected
project by comparing a filesystem path with the published identity digest, so
it never matched and fell through to "nothing selected". And the grepai line
read `Standby · waiting for its watcher` whenever the on-demand watcher had
idled out — which it does after fifteen minutes by design — although semantic
search was being served from the index the whole time. A complete index now
reads `Ready` and says the watcher starts on the next search; only a workspace
with no index is in standby, and its detail carries the `hzr index init`
command. A dashboard opened fresh lands on the workspace seen most recently.

## Pricing shows up, and says what it is

The public-list estimate was withheld whenever host delivery was unconfirmed,
which with no host acknowledging deliveries was always; the catalog was never
even named, so `claude-opus-5` showed as `Unavailable` while its entry sat in
the table. The estimate is now priced and carries a qualifier saying what
evidence is still missing, and the catalog identity is always reported.

## You can see which commands ran

Every operation now records a bounded, path-free summary — program, subcommand,
flags: `cargo test --locked`, `git commit -m`, `read --outline` — and never an
operand, so no path, query, secret or heredoc crosses. It shows on each activity
row and in a per-project **savings by command** table with runs, produced →
delivered tokens and the saved share, under a one-sentence brief: tools produced
N tokens, HZR handed the model M. `[privacy] publish_command_summaries = false`
withholds it. Rows written before 0.9.1 keep their family only.

## The dashboard carries the brand

The header's placeholder letter is replaced by the HZR logomark — one hollow
flame on an ink tile, the ember gradient of the illustration, legible at 16 px
in a browser tab. The
illustration itself sits behind the header as a masked ember overlay: only its
flame survives the mask, as warmth in the dark, and it carries no information so
assistive technology never sees it.

## Unchanged from 0.9.0

Identities are readable by default and can be withheld; the observer stays an
optional local build; Windows acceptance for it is still unverified.
