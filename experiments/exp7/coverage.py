#!/usr/bin/env python3
"""EXP-7: coverage of a fixed target word list by dictionary candidate files.

For every candidate of `candidates.json` the script verifies the SHA-256 of
the downloaded file, parses it, and looks every target word up twice: as
written, and through its dictionary forms (suffix rules plus the OEWN
exception list, no model). It writes one JSON line per word and candidate
plus one summary line per candidate, and prints the summary.

Standard library only. This is a throwaway measurement: the product reads
nothing from it. The reduction rules are restated in the experiment record so
that the product can implement the same rules.

Exit codes: 0 done; 2 a file's hash differs from the manifest; 3 the target
list is missing, not approved or changed after approval; 4 bad manifest.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import sys
import tarfile
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
RECORD = ROOT / "_bmad-output/initiative-linux-rust-translator/experiment-exp-7"

FIELDS = ("definitions", "synonyms", "translations_ru")

# What each format can carry at all. A field outside this set is reported as
# null ("not applicable"), never as zero: OEWN is not an EN->RU source.
PROVIDES = {
    "wn-lmf-xml-gz": ("definitions", "synonyms"),
    "freedict-tei-tar-xz": ("definitions", "translations_ru"),
    "freedict-dictd-tar-xz": ("definitions", "translations_ru"),
    "kaikki-jsonl-gz": ("definitions", "synonyms", "translations_ru"),
}


class HashMismatch(Exception):
    """The downloaded file is not the file the manifest describes."""


class ListNotApproved(Exception):
    """The target list is missing, not approved, or changed after approval."""


# --------------------------------------------------------------------------
# Reduction to the dictionary form
# --------------------------------------------------------------------------

# (suffix, replacement), tried in this order. Every rule that matches gives a
# form; the rules know no part of speech. A form is kept only when it is at
# least two letters long.
SUFFIX_RULES = (
    # plural and third person
    ("ies", "y"),
    ("ches", "ch"),
    ("shes", "sh"),
    ("sses", "ss"),
    ("xes", "x"),
    ("zes", "z"),
    ("oes", "o"),
    ("es", "e"),
    ("es", ""),
    ("s", ""),
    ("men", "man"),
    # past
    ("ied", "y"),
    ("ed", "e"),
    ("ed", ""),
    # -ing
    ("ying", "ie"),
    ("ing", "e"),
    ("ing", ""),
    # comparison
    ("ier", "y"),
    ("iest", "y"),
    ("er", "e"),
    ("er", ""),
    ("est", "e"),
    ("est", ""),
)

# After these suffixes are removed, a doubled final consonant gives one more
# form with a single consonant: stopped -> stopp -> stop.
DOUBLING_SUFFIXES = ("ed", "ing", "er", "est")
MIN_FORM = 2
VOWELS = "aeiou"


def suffix_forms(word: str) -> list[str]:
    """The forms the suffix rules give for `word`, in rule order, unique."""
    forms: list[str] = []

    def add(form: str) -> None:
        if len(form) >= MIN_FORM and form != word and form not in forms:
            forms.append(form)

    for suffix, replacement in SUFFIX_RULES:
        if word.endswith(suffix) and len(word) > len(suffix):
            add(word[: len(word) - len(suffix)] + replacement)
    for suffix in DOUBLING_SUFFIXES:
        if word.endswith(suffix):
            stem = word[: len(word) - len(suffix)]
            if len(stem) >= 3 and stem[-1] == stem[-2] and stem[-1] not in VOWELS:
                add(stem[:-1])
    return forms


def reduce_word(word: str, exceptions: dict[str, list[str]]) -> list[str]:
    """Dictionary forms to try for `word`: the exception list first (sorted),
    then the suffix rules. The word itself is never in the result."""
    forms: list[str] = []
    for form in sorted(exceptions.get(word, ())) + suffix_forms(word):
        if form != word and form not in forms:
            forms.append(form)
    return forms


# --------------------------------------------------------------------------
# Parsers. Each returns (index, stats[, exceptions]).
#   index: headword -> {"senses", "definitions", "synonyms", "translations_ru"}
#   stats: {"entries": int, "headwords": int}
# `wanted` limits the index to those headwords (None keeps all); the stats
# always count the whole file. Headwords are matched exactly, case included.
# --------------------------------------------------------------------------


def _blank() -> dict:
    return {"senses": 0, "definitions": 0, "synonyms": set(), "translations_ru": set()}


def _finish(index: dict) -> dict:
    return {
        word: {
            "senses": entry["senses"],
            "definitions": entry["definitions"],
            "synonyms": len(entry["synonyms"]),
            "translations_ru": len(entry["translations_ru"]),
        }
        for word, entry in index.items()
    }


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def parse_wn_lmf(stream, wanted=None):
    """WN-LMF XML (Open English WordNet). Also gives the exception list:
    the <Form> elements, irregular form -> lemmas."""
    entries = []  # (entry id, lemma, synset ids)
    lemma_of: dict[str, str] = {}
    synsets: dict[str, tuple[bool, list[str]]] = {}
    exceptions: dict[str, set[str]] = {}
    for _, element in ET.iterparse(stream, events=("end",)):
        tag = _local(element.tag)
        if tag == "LexicalEntry":
            lemma = None
            targets = []
            forms = []
            for child in element:
                child_tag = _local(child.tag)
                if child_tag == "Lemma":
                    lemma = child.get("writtenForm")
                elif child_tag == "Form":
                    forms.append(child.get("writtenForm"))
                elif child_tag == "Sense":
                    targets.append(child.get("synset"))
            if lemma is None:
                raise ValueError("LexicalEntry without a Lemma")
            entries.append((element.get("id"), lemma, targets))
            lemma_of[element.get("id")] = lemma
            for form in forms:
                if form and form != lemma:
                    exceptions.setdefault(form, set()).add(lemma)
            element.clear()
        elif tag == "Synset":
            definition = any(
                _local(child.tag) == "Definition" and (child.text or "").strip()
                for child in element
            )
            synsets[element.get("id")] = (definition, (element.get("members") or "").split())
            element.clear()
    if not entries:
        raise ValueError("no LexicalEntry element found")
    index: dict[str, dict] = {}
    headwords = set()
    for _, lemma, targets in entries:
        headwords.add(lemma)
        if wanted is not None and lemma not in wanted:
            continue
        entry = index.setdefault(lemma, _blank())
        for target in targets:
            entry["senses"] += 1
            definition, members = synsets.get(target, (False, []))
            if definition:
                entry["definitions"] += 1
            for member in members:
                other = lemma_of.get(member)
                if other is not None and other != lemma:
                    entry["synonyms"].add(other)
    stats = {"entries": len(entries), "headwords": len(headwords)}
    return _finish(index), stats, {form: sorted(lemmas) for form, lemmas in exceptions.items()}


def parse_freedict_tei(stream, wanted=None):
    """FreeDict TEI P5: <entry> with <orth>, top-level <sense> elements,
    <cit type="trans" xml:lang="ru"><quote> and nested <def>."""
    xml_lang = "{http://www.w3.org/XML/1998/namespace}lang"
    index: dict[str, dict] = {}
    headwords = set()
    count = 0
    for _, element in ET.iterparse(stream, events=("end",)):
        if _local(element.tag) != "entry":
            continue
        count += 1
        orths = [
            (node.text or "").strip() for node in element.iter() if _local(node.tag) == "orth"
        ]
        orths = [orth for orth in orths if orth]
        if not orths:
            raise ValueError("entry without <orth>")
        senses = sum(1 for child in element if _local(child.tag) == "sense")
        definitions = sum(
            1 for node in element.iter() if _local(node.tag) == "def" and (node.text or "").strip()
        )
        translations = set()
        for node in element.iter():
            if _local(node.tag) == "cit" and node.get("type") == "trans" and node.get(xml_lang) == "ru":
                for quote in node.iter():
                    if _local(quote.tag) == "quote" and (quote.text or "").strip():
                        translations.add(quote.text.strip())
        for orth in orths:
            headwords.add(orth)
            if wanted is not None and orth not in wanted:
                continue
            entry = index.setdefault(orth, _blank())
            entry["senses"] += senses
            entry["definitions"] += definitions
            entry["translations_ru"] |= translations
        element.clear()
    if not count:
        raise ValueError("no <entry> element found")
    return _finish(index), {"entries": count, "headwords": len(headwords)}


DICTD_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
# The info articles dictfmt 1.13.1 writes into the index, as the FreeDict
# eng-rus 2025.11.23 build has them. Only these exact headwords are skipped.
DICTD_INFO_HEADWORDS = frozenset(
    (
        "00databasealphabet",
        "00databasedictfmt1131",
        "00databaseinfo",
        "00databaseshort",
        "00databaseurl",
        "00databaseutf8",
    )
)
CYRILLIC = re.compile("[Ѐ-ӿ]")
NUMBER_ONLY = re.compile(r"^\s*\d+\.\s*$")
NUMBERED = re.compile(r"^\s*\d+\.\s")


def dictd_number(text: str) -> int:
    """A dictd index number: base 64, most significant digit first."""
    value = 0
    for character in text:
        value = value * 64 + DICTD_ALPHABET.index(character)
    return value


def parse_dictd(index_stream, dict_bytes: bytes, wanted=None):
    """dictd: `.index` (headword TAB offset TAB length) over the unpacked
    `.dict`. In an article the first line is the headword line; a later line
    with Cyrillic letters is a translation line, any other is a definition."""
    index: dict[str, dict] = {}
    headwords = set()
    count = 0
    for line in io.TextIOWrapper(index_stream, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line:
            continue
        parts = line.split("\t")
        if len(parts) < 3:
            raise ValueError(f"bad index line: {line!r}")
        headword, offset, length = parts[0], dictd_number(parts[1]), dictd_number(parts[2])
        if headword in DICTD_INFO_HEADWORDS:
            continue
        if offset + length > len(dict_bytes):
            raise ValueError(f"article of {headword!r} lies outside the dictionary file")
        count += 1
        headwords.add(headword)
        if wanted is not None and headword not in wanted:
            continue
        lines = dict_bytes[offset : offset + length].decode("utf-8").split("\n")[1:]
        entry = index.setdefault(headword, _blank())
        numbered = 0
        for text in lines:
            if not text.strip() or NUMBER_ONLY.match(text):
                continue
            if CYRILLIC.search(text):
                entry["translations_ru"].add(text.strip())
                if NUMBERED.match(text):
                    numbered += 1
            else:
                entry["definitions"] += 1
        entry["senses"] += max(numbered, 1)
    if not count:
        raise ValueError("no headword in the index")
    return _finish(index), {"entries": count, "headwords": len(headwords)}


def parse_kaikki(stream, wanted=None):
    """Kaikki/wiktextract JSONL, one entry per line; only entries of English
    words (`lang_code` "en") count. A sense marked `form_of` or `alt_of` only
    points at another word and is not a definition."""
    index: dict[str, dict] = {}
    headwords = set()
    count = 0
    for number, line in enumerate(stream, 1):
        # The raw dump holds every language: skip cheaply, then check.
        if '"en"' not in line:
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"line {number}: {error}") from error
        if not isinstance(record, dict):
            raise ValueError(f"line {number}: not an object")
        if record.get("lang_code") != "en":
            continue
        word = record.get("word")
        if not isinstance(word, str) or not isinstance(record.get("senses"), list):
            continue  # a redirect or a page without senses
        count += 1
        headwords.add(word)
        if wanted is not None and word not in wanted:
            continue
        entry = index.setdefault(word, _blank())
        holders = [record] + list(record["senses"])
        for sense in record["senses"]:
            entry["senses"] += 1
            pointer = "form_of" in sense or "alt_of" in sense
            if sense.get("glosses") and not pointer:
                entry["definitions"] += 1
        for holder in holders:
            for synonym in holder.get("synonyms", ()):
                if synonym.get("word") and synonym["word"] != word:
                    entry["synonyms"].add(synonym["word"])
            for translation in holder.get("translations", ()):
                code = translation.get("lang_code") or translation.get("code")
                if code == "ru" and translation.get("word"):
                    entry["translations_ru"].add(translation["word"])
    if not count:
        raise ValueError("no English entry found")
    return _finish(index), {"entries": count, "headwords": len(headwords)}


def _member(archive: tarfile.TarFile, name: str):
    try:
        handle = archive.extractfile(name)
    except KeyError:
        handle = None
    if handle is None:
        raise ValueError(f"no member {name!r} in the archive")
    return handle


def parse_candidate(candidate: dict, path: Path, wanted=None):
    """Parse one candidate file. Returns (index, stats, exceptions or None)."""
    kind = candidate["format"]
    members = candidate.get("members", {})
    if kind == "wn-lmf-xml-gz":
        with gzip.open(path, "rb") as stream:
            return parse_wn_lmf(stream, wanted)
    if kind == "freedict-tei-tar-xz":
        with tarfile.open(path, "r:xz") as archive:
            index, stats = parse_freedict_tei(_member(archive, members["tei"]), wanted)
        return index, stats, None
    if kind == "freedict-dictd-tar-xz":
        with tarfile.open(path, "r:xz") as archive:
            body = gzip.decompress(_member(archive, members["dict"]).read())
            index, stats = parse_dictd(_member(archive, members["index"]), body, wanted)
        return index, stats, None
    if kind == "kaikki-jsonl-gz":
        with gzip.open(path, "rt", encoding="utf-8") as stream:
            index, stats = parse_kaikki(stream, wanted)
        return index, stats, None
    raise ValueError(f"unknown format {kind!r}")


# --------------------------------------------------------------------------
# The run
# --------------------------------------------------------------------------


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_words(path: Path) -> list[tuple[str, str]]:
    """The target list as (word, section). `#` lines are comments; a
    `# [name]` line starts a section."""
    words: list[tuple[str, str]] = []
    seen = set()
    section = ""
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = line.strip()
        if not text:
            continue
        if text.startswith("#"):
            found = re.fullmatch(r"#\s*\[(.+)\]", text)
            if found:
                section = found.group(1)
            continue
        if not text.isalpha():
            raise ValueError(f"{path}:{number}: {text!r} is not a word of letters")
        if text in seen:
            raise ValueError(f"{path}:{number}: {text!r} is listed twice")
        seen.add(text)
        words.append((text, section))
    if not words:
        raise ValueError(f"{path}: no words")
    return words


def check_list(manifest: dict, path: Path) -> str:
    """The SHA-256 of the target list, if it is the approved one."""
    target = manifest.get("target_list") or {}
    approved = target.get("sha256")
    if not path.is_file():
        raise ListNotApproved(f"target list not found: {path}")
    actual = sha256_of(path)
    if not approved or not target.get("approved"):
        raise ListNotApproved(
            f"target list {path} is not approved: set target_list.sha256 and "
            f"target_list.approved in the manifest after the user approves it "
            f"(its SHA-256 now: {actual})"
        )
    if actual != approved:
        raise ListNotApproved(
            f"target list {path} changed after approval: approved {approved}, actual {actual}"
        )
    return actual


def verify_file(candidate: dict, path: Path) -> None:
    """Stop the run when a present file is not the one in the manifest."""
    size = path.stat().st_size
    actual = sha256_of(path)
    if size != candidate["size"]:
        raise HashMismatch(
            f"{path}: size differs from the manifest: expected {candidate['size']}, actual {size}; "
            f"SHA-256 expected {candidate['sha256']}, actual {actual}"
        )
    if actual != candidate["sha256"]:
        raise HashMismatch(
            f"{path}: SHA-256 differs from the manifest: "
            f"expected {candidate['sha256']}, actual {actual}"
        )


def look_up(word: str, forms: list[str], index: dict, provides) -> dict:
    """One word against one candidate: as written and with reduction."""
    own = index.get(word)
    via = [form for form in forms if form in index]

    def counts(entries: list[dict]) -> dict:
        row = {"senses": sum(entry["senses"] for entry in entries)}
        for field in FIELDS:
            row[field] = sum(entry[field] for entry in entries) if field in provides else None
        return row

    return {
        "found_as_written": own is not None,
        "found_via": via,
        "found_with_reduction": own is not None or bool(via),
        # Per form, for doubtful_forms.py; the figures above do not use it.
        "via_counts": {form: counts([index[form]]) for form in via},
        "as_written": counts([own] if own else []),
        "with_reduction": counts(([own] if own else []) + [index[form] for form in via]),
    }


def summarize(name: str, rows: list[dict], provides) -> dict:
    def figures(found_key: str, counts_key: str, subset: list[dict]) -> dict:
        result = {"found": sum(1 for row in subset if row[found_key])}
        for field in FIELDS:
            result[field] = (
                sum(1 for row in subset if row[counts_key][field]) if field in provides else None
            )
        return result

    sections: dict[str, dict] = {}
    for section in dict.fromkeys(row["section"] for row in rows):
        subset = [row for row in rows if row["section"] == section]
        sections[section] = {
            "words": len(subset),
            "as_written": figures("found_as_written", "as_written", subset),
            "with_reduction": figures("found_with_reduction", "with_reduction", subset),
        }
    return {
        "kind": "summary",
        "candidate": name,
        "status": "verified",
        "words": len(rows),
        "as_written": figures("found_as_written", "as_written", rows),
        "with_reduction": figures("found_with_reduction", "with_reduction", rows),
        "by_section": sections,
        "missed_as_written": [row["word"] for row in rows if not row["found_as_written"]],
        "missed_with_reduction": [row["word"] for row in rows if not row["found_with_reduction"]],
        "found_without_field_with_reduction": {
            field: (
                [
                    row["word"]
                    for row in rows
                    if row["found_with_reduction"] and not row["with_reduction"][field]
                ]
                if field in provides
                else None
            )
            for field in FIELDS
        },
    }


def run(manifest: dict, words: list[tuple[str, str]], data: Path, list_sha256=None) -> list[dict]:
    """All result lines, in a fixed order. Raises HashMismatch before any
    candidate is parsed when a present file is not the manifest's file."""
    candidates = manifest["candidates"]
    names = [candidate["name"] for candidate in candidates]
    if len(set(names)) != len(names):
        raise ValueError("candidate names are not unique")

    state: dict[str, tuple[str, str | None]] = {}
    for candidate in candidates:
        name = candidate["name"]
        if candidate.get("status") == "unverified":
            state[name] = ("unverified", candidate.get("reason") or "marked unverified in the manifest")
        elif candidate.get("format") not in PROVIDES:
            state[name] = ("unverified", f"unknown format {candidate.get('format')!r}")
        elif not (data / candidate["file"]).is_file():
            state[name] = ("unverified", f"file not found: {candidate['file']}")
        else:
            verify_file(candidate, data / candidate["file"])
            state[name] = ("verified", None)

    # The exception list comes first: every candidate is reduced with it.
    exceptions: dict[str, list[str]] = {}
    exception_source = None
    parsed: dict[str, tuple[dict, dict]] = {}
    for candidate in candidates:
        name = candidate["name"]
        if state[name][0] != "verified" or not candidate.get("exception_source"):
            continue
        try:
            index, stats, found = parse_candidate(candidate, data / candidate["file"])
        except Exception as error:  # an unparsable file is recorded, the run goes on
            state[name] = ("unverified", f"cannot parse: {type(error).__name__}: {error}")
            continue
        parsed[name] = (index, stats)
        if exception_source is None and found is not None:
            exceptions, exception_source = found, name

    forms = {word: reduce_word(word, exceptions) for word, _ in words}
    wanted = {word for word, _ in words} | {form for found in forms.values() for form in found}

    lines: list[dict] = [
        {
            "kind": "run",
            "schema": 1,
            "target_list_sha256": list_sha256,
            "words": len(words),
            "exception_source": exception_source,
            "exceptions": len(exceptions),
            "matching": "exact headword, case included",
        }
    ]
    summaries = []
    for candidate in candidates:
        name = candidate["name"]
        if state[name][0] == "verified" and name not in parsed:
            try:
                index, stats, _ = parse_candidate(candidate, data / candidate["file"], wanted)
                parsed[name] = (index, stats)
            except Exception as error:
                state[name] = ("unverified", f"cannot parse: {type(error).__name__}: {error}")
        status, reason = state[name]
        head = {
            "kind": "candidate",
            "candidate": name,
            "status": status,
            "reason": reason,
            "file": candidate.get("file"),
            "url": candidate.get("url"),
            "version": candidate.get("version"),
            "size": candidate.get("size"),
            "sha256": candidate.get("sha256"),
            "format": candidate.get("format"),
        }
        if status != "verified":
            lines.append(head)
            summaries.append(
                {"kind": "summary", "candidate": name, "status": "unverified", "reason": reason}
            )
            continue
        index, stats = parsed[name]
        provides = PROVIDES[candidate["format"]]
        head.update(stats)
        head["provides"] = list(provides)
        lines.append(head)
        rows = []
        for word, section in words:
            row = {"kind": "word", "candidate": name, "word": word, "section": section}
            row.update(look_up(word, forms[word], index, provides))
            rows.append(row)
        lines.extend(rows)
        summaries.append(summarize(name, rows, provides))
    return lines + summaries


def render(lines: list[dict]) -> str:
    return "".join(
        json.dumps(line, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n"
        for line in lines
    )


def print_summary(lines: list[dict], out=None) -> None:
    out = out or sys.stdout
    def cell(value, total):
        return "n/a" if value is None else f"{value}/{total}"

    print(f"{'candidate':<24} {'lookup':<15} {'found':>9} {'defs':>9} {'syn':>9} {'ru':>9}", file=out)
    for line in lines:
        if line["kind"] != "summary":
            continue
        if line["status"] != "verified":
            print(f"{line['candidate']:<24} UNVERIFIED: {line['reason']}", file=out)
            continue
        total = line["words"]
        for key, label in (("as_written", "as written"), ("with_reduction", "with reduction")):
            figures = line[key]
            print(
                f"{line['candidate']:<24} {label:<15} {cell(figures['found'], total):>9} "
                f"{cell(figures['definitions'], total):>9} {cell(figures['synonyms'], total):>9} "
                f"{cell(figures['translations_ru'], total):>9}",
                file=out,
            )
        missed = line["missed_with_reduction"]
        print(f"{'':<24} missed with reduction ({len(missed)}): {' '.join(missed) or '-'}", file=out)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--candidates", type=Path, default=HERE / "candidates.json")
    parser.add_argument("--data", type=Path, default=HERE / "data")
    parser.add_argument("--words", type=Path, default=None)
    parser.add_argument("--out", type=Path, default=RECORD / "results.jsonl")
    arguments = parser.parse_args(argv)

    try:
        manifest = json.loads(arguments.candidates.read_text(encoding="utf-8"))
        manifest["candidates"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: bad manifest {arguments.candidates}: {error}", file=sys.stderr)
        return 4
    words_path = arguments.words
    if words_path is None:
        words_path = ROOT / (manifest.get("target_list") or {}).get("path", "")
    try:
        list_sha256 = check_list(manifest, words_path)
        words = read_words(words_path)
    except (ListNotApproved, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 3
    try:
        lines = run(manifest, words, arguments.data, list_sha256)
    except HashMismatch as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    except (KeyError, ValueError) as error:
        print(f"error: bad manifest {arguments.candidates}: {error!r}", file=sys.stderr)
        return 4

    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = arguments.out.with_name(arguments.out.name + ".tmp")
    temporary.write_text(render(lines), encoding="utf-8")
    os.replace(temporary, arguments.out)
    print_summary(lines)
    print(f"written: {arguments.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
