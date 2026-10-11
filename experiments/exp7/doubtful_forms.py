#!/usr/bin/env python3
"""EXP-7: how much the part-of-speech-blind reduction pollutes the figures.

Reads results.jsonl and a Kaikki file. A dictionary form that a word was
found through is "confirmed" when the Kaikki entry of the word has a sense
marked `form_of` that names this form (went -> go), and "doubtful" otherwise
(better -> bet, letter -> let). Prints, per candidate: the words that carry a
doubtful form; the words found ONLY through doubtful forms -- what inflates
the "found with reduction" figure; and, per field, the words whose field
comes ONLY from doubtful forms -- what inflates that field's figure.

Standard library only; changes nothing.
"""

import argparse
import gzip
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from coverage import FIELDS, RECORD  # noqa: E402


def form_of(path: Path, words: set[str]) -> dict[str, set[str]]:
    """word -> the words its English `form_of` senses name."""
    found: dict[str, set[str]] = {word: set() for word in words}
    with gzip.open(path, "rt", encoding="utf-8") as stream:
        for line in stream:
            if '"form_of"' not in line or '"en"' not in line:
                continue
            record = json.loads(line)
            if record.get("lang_code") != "en" or record.get("word") not in words:
                continue
            for sense in record.get("senses", ()):
                for target in sense.get("form_of", ()):
                    if target.get("word"):
                        found[record["word"]].add(target["word"])
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--results", type=Path, default=RECORD / "results.jsonl")
    parser.add_argument("--kaikki", type=Path, default=HERE / "data/raw-wiktextract-data.jsonl.gz")
    arguments = parser.parse_args()

    rows = [
        json.loads(line)
        for line in arguments.results.read_text(encoding="utf-8").splitlines()
    ]
    rows = [row for row in rows if row["kind"] == "word"]
    confirmed = form_of(arguments.kaikki, {row["word"] for row in rows if row["found_via"]})
    for candidate in dict.fromkeys(row["candidate"] for row in rows):
        own = [row for row in rows if row["candidate"] == candidate]
        reduced = [row for row in own if row["found_via"]]
        polluted, inflating = [], []
        by_field: dict[str, list[str]] = {field: [] for field in FIELDS}
        for row in reduced:
            doubtful = [form for form in row["found_via"] if form not in confirmed[row["word"]]]
            if not doubtful:
                continue
            polluted.append(f"{row['word']}>{'/'.join(doubtful)}")
            if not row["found_as_written"] and len(doubtful) == len(row["found_via"]):
                inflating.append(f"{row['word']}>{'/'.join(doubtful)}")
            for field in FIELDS:
                if row["as_written"][field] or not row["with_reduction"][field]:
                    continue
                givers = [form for form in row["found_via"] if row["via_counts"][form][field]]
                if all(form in doubtful for form in givers):
                    by_field[field].append(f"{row['word']}>{'/'.join(givers)}")
        print(f"{candidate}: {len(reduced)} of {len(own)} words have a reduced form in the file")
        print(f"  with a doubtful form ({len(polluted)}): {' '.join(polluted) or '-'}")
        if all(row["found_as_written"] for row in own):
            print("  found only through doubtful forms: not applicable, every word is found as written")
        else:
            print(f"  found only through doubtful forms ({len(inflating)}): {' '.join(inflating) or '-'}")
        for field in FIELDS:
            if own[0]["with_reduction"][field] is None:
                print(f"  {field} only from doubtful forms: not applicable, the format has no such field")
                continue
            measured = sum(1 for row in own if row["with_reduction"][field])
            names = by_field[field]
            print(
                f"  {field} only from doubtful forms ({len(names)}; measured {measured}, "
                f"without them {measured - len(names)}): {' '.join(names) or '-'}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
