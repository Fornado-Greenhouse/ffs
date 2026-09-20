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

## Multi-entity and identity fixtures (task_45)

Fixtures `21` to `25` are synthetic business-press articles (an
executive hire, a funding round, an acquisition, a real-estate
opening, a profile) whose expectations only a model can meet; they
are marked `"engine": "llm"` and carry the canned envelope a capable
model produced as `model_output.json`. `test_corpus.py` skips them
unless `FFS_SCRIBE_ENGINE=llm` and a backend answers;
`test_multi_entity.py` feeds the canned envelope through `LlmEngine`
with a fake transport, so they are exercised in CI with no network.
Their expectations may pin `refs_count` (bound cross-references) and
`top` keys (`ends_role`, `valid_from`, `valid_to`).

Fixtures `31` to `34` are identity fixtures: several articles in one
directory (`input.md`, `input-b.md`, `input-c.md`, each with its
`model_output[-x].json`) and an `expected_identity.json` naming the
expected clusters (`P1`, `P2`, `O1`), the pairs that must stay
distinct, the expected alias growth, a rename, and a wrong merge with
its undo. The Python tests check extraction and reference binding per
article; the daemon's resolver tests consume the clustering and score
it with pairwise precision/recall and B-cubed.
