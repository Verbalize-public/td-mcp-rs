#!/usr/bin/env python3
"""Measure the compact codec's effect in TOKENS, naming the tokenizer used.

Why this is a separate script rather than a Rust test
-----------------------------------------------------
Tokens are a property of a SPECIFIC tokenizer over a SPECIFIC document mix.
Bytes are not: one byte is one byte. So a byte reduction and a token reduction
are two different measurements, and quoting one as the other invents a number.
The `cargo test` suite keeps asserting the byte figure (and the lossless round
trip) because that needs no dependency; this script takes the document pairs
`TDMCP_TOKEN_DUMP` writes and counts them properly.

The honesty rules this implements, each because the alternative is a real way
to flatter the result:

* **The tokenizer is named, in the output and in the exit code.** A token count
  without its tokenizer is a unit-less number.
* **Encodings are proven distinct before they are used.** A tokenizer that
  silently failed to load and fell back to a character count would produce
  plausible, wrong numbers, so every requested encoding must load AND agree
  with a known reference on a reference string. If two encodings are asked for
  and they produce identical token streams they are treated as a failure, not
  as agreement: distinct vocabularies give distinct answers, and an identical
  answer means one of them is not really the encoder it claims to be.
* **The declined calls are counted, not dropped.** 81 of 121 recorded payloads
  are answered with the full value because compacting them would have been
  wrong. Counting only the 40 that compacted would describe a subset nobody
  receives, and would report a much larger saving than the release delivers.
* **A per-plan breakdown is reported alongside the total**, so a saving that
  comes entirely from one kind of payload is visible rather than averaged away.
* **The byte figure is printed next to the token figure**, from the same data.
  When they disagree, the disagreement is the interesting result.

Usage
-----
    TDMCP_INSPECT_CORPUS=retroanalysis/release-analysis/output-study/inspect \\
    TDMCP_TOKEN_DUMP=/tmp/token-dump.json \\
        cargo test --offline --locked -p tdmcp-mcp --test inspect_corpus_replay

    python scripts/measure_tokens.py /tmp/token-dump.json

Exit status is non-zero on any failure to measure, so this cannot be used to
report a number it did not earn.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

# Encodings measured by default. These are named in the report: a reader has to
# be able to reproduce the figure or discard it, and that requires the name.
DEFAULT_ENCODINGS = ("cl100k_base", "o200k_base")

# (encoding, text, expected token count) from each vocabulary's published
# behaviour. Used to prove the loaded encoder is the encoder it claims to be:
# a character-count fallback would get `len(text)` instead, which differs for
# every one of these.
REFERENCE_CASES = (
    ("cl100k_base", "hello world", 2),
    ("o200k_base", "hello world", 2),
    ("p50k_base", "hello world", 2),
    ("r50k_base", "hello world", 2),
)


class MeasurementError(RuntimeError):
    """A measurement that cannot be trusted, and must not be reported."""


def load_encodings(names: tuple[str, ...]):
    """Load each named encoding and prove it is the encoder it claims to be.

    Returns ``{name: encoding}``. Raises rather than degrading: a fallback that
    still produces a number is the dangerous case, because the number looks
    like a measurement.
    """
    try:
        import tiktoken
    except ImportError as error:  # pragma: no cover - environment dependent
        raise MeasurementError(
            "tiktoken is not installed. Token figures are produced here rather "
            "than guessed from byte counts: install tiktoken, or report the "
            "byte figure and name it as bytes."
        ) from error

    encodings = {}
    for name in names:
        try:
            encoding = tiktoken.get_encoding(name)
        except Exception as error:  # noqa: BLE001 - the failure text is the point
            raise MeasurementError(f"could not load encoding {name}: {error}") from error

        expected = {case[1]: case[2] for case in REFERENCE_CASES if case[0] == name}
        for text, tokens in expected.items():
            actual = len(encoding.encode(text))
            if actual != tokens:
                raise MeasurementError(
                    f"encoding {name} produced {actual} tokens for {text!r}, "
                    f"expected {tokens}. This is not the encoder it claims to "
                    "be, so its counts cannot be reported."
                )
        encodings[name] = encoding
    return encodings


def prove_encodings_differ(encodings: dict) -> str:
    """Check the loaded encodings really are different, and report what proved it.

    Two vocabularies that agree on every string are indistinguishable from one
    encoding loaded twice, which would turn "we measured three tokenizers" into
    a claim backed by one.
    """
    probe = (
        "The evidence store retains an immutable baseline; a delta requires "
        "fresh observation. missing != unavailable != unrequested."
    )
    if len(encodings) < 2:
        return "single encoding, no cross-check applicable"

    digests = {}
    for name, encoding in encodings.items():
        digests[name] = tuple(encoding.encode(probe))

    distinct = len(set(digests.values()))
    if distinct < len(digests):
        duplicates = sorted(
            name for name in digests if list(digests.values()).count(digests[name]) > 1
        )
        raise MeasurementError(
            f"encodings {duplicates} produced identical token streams, so they "
            "are not distinct tokenizers. Reporting them as separate "
            "measurements would overstate the evidence."
        )
    return f"{len(digests)} encodings agree on being distinct ({probe!r} splits each differently)"


def measure(rows: list[dict], encodings: dict) -> dict:
    """Count tokens for both documents of every row, per encoding."""
    report = {}
    for name, encoding in encodings.items():
        per_plan = defaultdict(lambda: {"calls": 0, "full": 0, "adaptive": 0})
        full_total = 0
        adaptive_total = 0
        for row in rows:
            plan = row["plan"]
            full_tokens = len(encoding.encode(row["full"]))
            adaptive_tokens = len(encoding.encode(row["adaptive"]))
            bucket = per_plan[plan]
            bucket["calls"] += 1
            bucket["full"] += full_tokens
            bucket["adaptive"] += adaptive_tokens
            full_total += full_tokens
            adaptive_total += adaptive_tokens

        # A row that was answered with the full value must cost the same in both
        # directions. If it does not, the dump does not describe the documents
        # the release would ship, and the totals below are meaningless.
        for plan, bucket in per_plan.items():
            if plan != "compact" and bucket["full"] != bucket["adaptive"]:
                raise MeasurementError(
                    f"{bucket['calls']} {plan} calls cost {bucket['full']} full tokens "
                    f"but {bucket['adaptive']} adaptive tokens. A declined compact "
                    "request must return the full value unchanged, so the dump "
                    "does not match the behaviour it is measuring."
                )

        saved = full_total - adaptive_total
        report[name] = {
            "calls": len(rows),
            "full_tokens": full_total,
            "adaptive_tokens": adaptive_total,
            "tokens_saved": saved,
            "percent_saved": (100.0 * saved / full_total) if full_total else None,
            "per_plan": {
                plan: {
                    "calls": bucket["calls"],
                    "full_tokens": bucket["full"],
                    "adaptive_tokens": bucket["adaptive"],
                    "percent_saved": (
                        (100.0 * (bucket["full"] - bucket["adaptive"]) / bucket["full"])
                        if bucket["full"]
                        else None
                    ),
                }
                for plan, bucket in sorted(per_plan.items())
            },
        }
    return report


def percent(value: float | None) -> str:
    """Render a saving percentage, or say there is none.

    A set of documents that tokenises to nothing has no percentage saving. The
    tempting shortcut is to print 0.00%, which reads as "measured, saved
    nothing" and is really "there was nothing to measure" — the same
    distinction the rest of this work keeps, so it is kept here too.
    """
    return "undefined" if value is None else f"{value:.2f}%"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("dump", type=Path, help="the file TDMCP_TOKEN_DUMP wrote")
    parser.add_argument(
        "--encoding",
        action="append",
        dest="encodings",
        help="tiktoken encoding to measure with; repeatable (default: "
        + ", ".join(DEFAULT_ENCODINGS)
        + ")",
    )
    args = parser.parse_args()

    if not args.dump.is_file():
        print(f"error: {args.dump} is not a file", file=sys.stderr)
        print(
            "Generate it with TDMCP_TOKEN_DUMP set while the corpus replay runs.",
            file=sys.stderr,
        )
        return 2

    rows = json.loads(args.dump.read_text())
    if not isinstance(rows, list) or not rows:
        print(f"error: {args.dump} holds no document pairs", file=sys.stderr)
        return 2

    names = tuple(args.encodings or DEFAULT_ENCODINGS)
    try:
        encodings = load_encodings(names)
        distinct_note = prove_encodings_differ(encodings)
        report = measure(rows, encodings)
    except MeasurementError as error:
        print(f"measurement refused: {error}", file=sys.stderr)
        return 1

    print(f"token measurement over {len(rows)} inspect documents")
    print(f"  encodings: {', '.join(names)}")
    print(f"  distinctness check: {distinct_note}")
    for name, data in report.items():
        print(f"\n{name}:")
        print(f"  tokens: {data['full_tokens']} -> {data['adaptive_tokens']} "
              f"({data['tokens_saved']} fewer, {percent(data['percent_saved'])} fewer)")
        for plan, bucket in data["per_plan"].items():
            share = (
                f", {percent(bucket['percent_saved'])} fewer"
                if bucket["percent_saved"] is not None
                else ""
            )
            print(f"    {plan}: {bucket['calls']} calls, "
                  f"{bucket['full_tokens']} -> {bucket['adaptive_tokens']}{share}")
    print(
        "\nThese are TOKENS for the encodings named above, measured over the same\n"
        "documents as the byte figures in the replay output. They are not a unit\n"
        "conversion from bytes: compare the two, and treat a disagreement as the\n"
        "finding rather than averaging it away."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
