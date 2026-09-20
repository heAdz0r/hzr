# Synthetic example: invoice status filtering

This is an illustrative manifest/brief, not a discovered feature in your repo.
Assume a small Python application already defines immutable Invoice records with
id and status fields; status is open, paid, or void. A filtering helper must return
a new list of matching records, retain order and identity, and reject unknown
filter values. An absent filter returns all records without mutating input.
There are no payment-provider calls, private data, network access, or UI changes.
The intended code/test paths in this example are illustrative and do not exist
in this package. The manifest linter validates structure, not implementability.
