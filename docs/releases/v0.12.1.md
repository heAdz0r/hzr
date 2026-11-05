# HZR 0.12.1

`cat` returns the exact bytes of non-UTF-8 and CRLF files even when they were read
before the upgrade to 0.12.0.

## What broke

0.12.0 made `cat` byte-exact for files that are not valid UTF-8 or use CRLF line
endings. `rtk read` still looked in its read cache first, and entries written by
0.11.x held the decoded text: `U+FFFD` in place of invalid bytes and LF in place of
CRLF. Those entries have no expiry and kept the same key, so a file read before the
upgrade came back altered until it changed on disk.

## What changed

- The read-cache format moves to version 6, so entries written before 0.12.1 are
  no longer used.
- A cached answer is ignored whenever the file's bytes must be returned exactly.

## Upgrade

Run `hzr update` or use the installer. No data migration is needed; old cache
entries are simply not read again.
