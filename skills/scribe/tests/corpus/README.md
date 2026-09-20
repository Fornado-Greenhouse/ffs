# Scribe golden corpus

Twenty synthetic fixtures that pin the scribe's extraction behavior
(task_36, ADR-026). Every name, business, and event here is invented.
No press text, no real newsletter content, nothing from a spike
directory may enter this corpus. Real articles used for local scoring
live under `$FFS_DATA_DIR` and are pointed at with
`FFS_SCRIBE_CORPUS_DIR`.

Each fixture directory holds:

- `input.md`: the note as it would be dropped into `ingest/`.
- `expected.json`: the original `filename`, the expected `proposals`
  (only the scored fields), optional `soft` fields (reported, not
  scored), optional `forbid` (predicates that must not appear),
  `required` (gates the task), `awaits` (the subtask a red fixture
  waits on), and `expected_when_registered` for fixtures whose
  expectation flips once a later task registers a predicate.

Run the heuristic scores as part of the normal test run:

```sh
.venv/bin/python -m pytest skills/scribe/tests/test_corpus.py
```

Score another engine or another corpus out of band:

```sh
FFS_SCRIBE_ENGINE=llm FFS_SCRIBE_CORPUS_DIR=~/.ffs/corpus \
  .venv/bin/python skills/scribe/tests/corpus_scorer.py
```

The scorer is `corpus_scorer.py`: `load_corpus`, `run_engine`,
`score`, `summarize`, `format_table`, plus the two schema helpers
tests use (`starter_registry_env`, `install_query_stub`).
