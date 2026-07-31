# TODOLIST

## Tighten fuzzy near-miss suggestions (diagnostics)

`bridge/tdmcp_bridge/__init__.py` → `_suggest_names` uses `difflib.get_close_matches(..., cutoff=0.5)` over `dir(td)`. Shared family suffixes (`TOP`/`CHOP`/…) inflate ratios, so garbage inputs (`fooTOP`, `xyzTOP`, `geo`) get confident wrong `tdmcp.op.similar_type` / `tdmcp.par.similar_name` lints.

**Fix direction:** score stems (strip family), require same family when present, stem/prefix gates for bare names, length/edit budget, filter `list_op_type_names` to real op classes; keep casefold + real typos (`hsvAdjustTOP`, `noizeTOP`, `satmult`). Extend `bridge/tests/test_mutate.py`.
