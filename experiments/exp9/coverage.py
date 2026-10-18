#!/usr/bin/env python3
"""EXP-9b: coverage of the approved ES and CA target lists by the candidate
files of experiments/exp9/candidates.json, per content kind, as written and
through the ES/CA dictionary-form rules of rules.py (rules only; rules plus
the irregular-only exception list; rules plus the full form -> lemma table,
each exception list from LanguageTool and from Wiktionary, measured
separately), and false hits across EN, ES and CA.

Run from the repository root:

    MORFOLOGIK_CP='/path/to/morfologik/jars/*' python3 experiments/exp9/coverage.py

Before anything is parsed, the hash of each target list (the two of
`target_lists` and EXP-7's list) and the size and SHA-256 of every file of
both manifests are checked; a difference stops the run with both hashes
printed. Without MORFOLOGIK_CP the LanguageTool exception lists are skipped
and the run line and the summary say so.

Writes _bmad-output/initiative-linux-rust-translator/experiment-exp-9/results.jsonl
(sorted JSON lines: `run`, `exceptions`, `candidate`, `word`, `summary`,
`language`, `false_hits`) and prints a summary. Standard library only; the
product reads nothing from it.

    python3 experiments/exp9/coverage.py --chosen es=NAME,NAME --chosen ca=NAME

recomputes, from results.jsonl alone, coverage and false hits for a chosen
set of candidates per language, and prints them as JSON lines.

Exit codes: 0 done; 2 a file differs from its manifest; 3 a target list is
missing or differs from its approved hash; 4 bad manifest or arguments.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import importlib.util
import json
import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

import rules  # noqa: E402
import sources  # noqa: E402

RECORD = ROOT / "_bmad-output/initiative-linux-rust-translator/experiment-exp-9"
MANIFEST = HERE / "candidates.json"
DATA = HERE / "data"
EXP7 = ROOT / "experiments/exp7"
LANGS = ("es", "ca")
KINDS = sources.KINDS

# The exception sources per language. `languagetool` needs MORFOLOGIK_CP;
# `wiktionary` is the union of the other (Wiktionary) sources.
FORM_SOURCES = {
    "es": {
        "languagetool": "spanish-pos-dict-2.5",
        "wiktionary": ["kaikki-en-Spanish", "kaikki-es-Espanol"],
    },
    "ca": {
        "languagetool": "catalan-pos-dict-3.3",
        "wiktionary": ["kaikki-en-Catalan", "kaikki-es-Catalan", "wikdict-ca", "dbnary-ca-morphology"],
    },
}
EN_SET = ("oewn-2025", "freedict-eng-rus-tei")  # EXP-7's chosen set


def load_exp7():
    spec = importlib.util.spec_from_file_location("exp7_coverage", EXP7 / "coverage.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


exp7 = load_exp7()


class Stop(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


# --------------------------------------------------------------------------
# Checks before parsing
# --------------------------------------------------------------------------


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 22), b""):
            digest.update(block)
    return digest.hexdigest()


def check_list(path: Path, approved: str | None) -> str:
    if not path.is_file():
        raise Stop(3, f"target list not found: {path}")
    actual = sha256_of(path)
    if not approved or actual != approved:
        raise Stop(3, f"target list {path} differs from its approved hash: expected {approved}, actual {actual}")
    return actual


def check_file(entry: dict, data: Path) -> Path:
    path = Path(os.path.normpath(data / entry["file"]))
    if not path.is_file():
        raise Stop(2, f"{path}: missing; expected size {entry['size']}, SHA-256 {entry['sha256']}")
    size = path.stat().st_size
    actual = sha256_of(path)
    if size != entry["size"] or actual != entry["sha256"]:
        raise Stop(
            2,
            f"{path}: differs from the manifest: size expected {entry['size']}, actual {size}; "
            f"SHA-256 expected {entry['sha256']}, actual {actual}",
        )
    return path


def read_list(path: Path) -> list[tuple[str, str]]:
    """(word, section) of an ES/CA list. Unlike EXP-7's reader it allows an
    inner `'` or `·` between letters."""
    words, seen, section = [], set(), ""
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = line.strip()
        if not text:
            continue
        if text.startswith("#"):
            found = re.fullmatch(r"#\s*\[(.+)\]", text)
            if found:
                section = found.group(1)
            continue
        if not rules.is_word(text):
            raise Stop(3, f"{path}:{number}: {text!r} is not a word")
        if text in seen:
            raise Stop(3, f"{path}:{number}: {text!r} is listed twice")
        seen.add(text)
        words.append((text, section))
    return words


# --------------------------------------------------------------------------
# Exception lists
# --------------------------------------------------------------------------


def table_stats(pairs, lang):
    """Size of the full table and of its irregular-only part, over the whole
    source (not only the list words)."""
    by_form = collections.defaultdict(set)
    for form, lemma in pairs:
        by_form[form].add(lemma)
    irregular_pairs = 0
    irregular_forms = 0
    for form, lemmas in by_form.items():
        reached = set(rules.rule_forms(form, lang))
        kept = sum(1 for lemma in lemmas if lemma not in reached)
        irregular_pairs += kept
        irregular_forms += bool(kept)
    return by_form, {
        "full_pairs": sum(len(lemmas) for lemmas in by_form.values()),
        "full_forms": len(by_form),
        "irregular_pairs": irregular_pairs,
        "irregular_forms": irregular_forms,
    }


def variants_for(by_form, keys, lang):
    """The full and the irregular-only exception lists, cut to `keys`."""
    full, irregular = {}, {}
    for key in keys:
        lemmas = by_form.get(key)
        if not lemmas:
            continue
        full[key] = sorted(lemmas)
        reached = set(rules.rule_forms(key, lang))
        kept = sorted(lemma for lemma in lemmas if lemma not in reached)
        if kept:
            irregular[key] = kept
    return full, irregular


def read_form_sources(lang, names, rows_by_name, paths, classpath, skipped, cache_kaikki, cache_dbnary):
    """name -> (form, lemma) pairs of each exception source. A LanguageTool
    source without MORFOLOGIK_CP is left out and named in `skipped`, never
    given as an empty list."""
    pairs_by_source = {}
    for name in names:
        row = rows_by_name[name]
        log(f"forms {lang}: {name} ...")
        if row["format"] == "morfologik-jar":
            if not classpath:
                skipped[name] = "MORFOLOGIK_CP not set: LanguageTool exception list skipped"
                log(f"  skipped: {skipped[name]}")
                continue
            pairs = sources.morfologik_forms(paths[name], row, classpath)
        elif row["format"] == "kaikki-jsonl-gz":
            index, pairs = sources.kaikki(paths[name], row, lang, forms=True)
            cache_kaikki[(name, lang)] = index
        elif row["format"] == "wikdict-sqlite-mono":
            pairs = sources.wikdict_forms(paths[name], row, lang)
        elif row["format"] == "dbnary-ttl-bz2":
            ontolex = "dbnary-ca-ontolex"
            if ontolex not in cache_dbnary:
                cache_dbnary[ontolex] = sources.dbnary_entries(paths[ontolex], lang)
            pairs = sources.dbnary_forms(paths[name], cache_dbnary[ontolex][0])
        else:
            raise Stop(4, f"{name}: no form reader for format {row['format']}")
        pairs_by_source[name] = pairs
    return pairs_by_source


# --------------------------------------------------------------------------
# Lookup
# --------------------------------------------------------------------------


def kinds_of(entries, provided):
    """Item counts per kind the candidate provides; a kind it does not
    provide is left out (and reported as null in the summaries)."""
    return {kind: sum(entry.get(kind, 0) for entry in entries) for kind in KINDS if kind in provided}


def confirmers(word, lang, table):
    """The forms through which LanguageTool's full table confirms a hit of
    `word`: its lemmas, each base and the lemmas of each base. None without
    the table (LanguageTool skipped)."""
    if table is None:
        return None
    confirm = set(table.get(word, ()))
    for base in rules.strip_bases(word, lang):
        confirm.add(base)
        confirm.update(table.get(base, ()))
    return confirm


def look_up(word, index, variant_forms, provided, confirm=None):
    """One word against one index: as written and per variant. A variant
    hit is `confirmed` when one of its forms is in `confirm` (None: not
    checked)."""
    own = index.get(word)
    row = {"as_written": {"found": own is not None, "kinds": kinds_of([own] if own else [], provided)}}
    variants = {}
    for name, forms in variant_forms.items():
        via = [form for form in forms if form in index]
        if not via:
            continue
        entries = ([own] if own else []) + [index[form] for form in via]
        confirmed = None if confirm is None else any(form in confirm for form in via)
        variants[name] = {"via": via, "kinds": kinds_of(entries, provided), "confirmed": confirmed}
    row["variants"] = variants
    return row


def unconfirmed(row, variant):
    """Whether the row is found under `variant` only through forms that
    LanguageTool does not give for it (None: not checked)."""
    if variant == "as_written" or row["as_written"]["found"] or variant not in row["variants"]:
        return False
    confirmed = row["variants"][variant].get("confirmed")
    return None if confirmed is None else not confirmed


def figures(rows, variant, provided):
    """found and per-kind counts of `rows` under `variant`, and how many of
    the found were reached only through unconfirmed forms."""
    result = {"found": 0, "unconfirmed": 0}
    for kind in KINDS:
        result[kind] = 0 if kind in provided else None
    for row in rows:
        found, kinds = resolve(row, variant)
        result["found"] += found
        flag = unconfirmed(row, variant)
        if flag is None:
            result["unconfirmed"] = None
        elif result["unconfirmed"] is not None:
            result["unconfirmed"] += flag
        for kind in KINDS:
            if kind in provided and kinds.get(kind):
                result[kind] += 1
    return result


def resolve(row, variant):
    """(found, kinds) of a word row under a variant."""
    if variant != "as_written" and variant in row["variants"]:
        return True, row["variants"][variant]["kinds"]
    return row["as_written"]["found"], row["as_written"]["kinds"]


def summarize(name, lang, rows, variant_names, provided, kind="summary"):
    sections = list(dict.fromkeys(row["section"] for row in rows))
    summary = {
        "kind": kind,
        "candidate": name,
        "lang": lang,
        "words": len(rows),
        "provides": [k for k in KINDS if k in provided],
        "variants": {},
    }
    for variant in variant_names:
        block = figures(rows, variant, provided)
        block["by_section"] = {}
        for section in sections:
            subset = [row for row in rows if row["section"] == section]
            block["by_section"][section] = dict(figures(subset, variant, provided), words=len(subset))
        block["missed"] = [row["word"] for row in rows if not resolve(row, variant)[0]]
        flags = [(row["word"], unconfirmed(row, variant)) for row in rows]
        block["unconfirmed_words"] = (
            None if any(flag is None for _, flag in flags) else [word for word, flag in flags if flag]
        )
        block["missing_kind"] = {
            k: [row["word"] for row in rows if not resolve(row, variant)[1].get(k)] if k in provided else None
            for k in KINDS
        }
        summary["variants"][variant] = block
    return summary


def union_rows(rows_by_candidate, lang, words):
    """Per word, the union over candidates: found if any finds it; a kind
    counts if any candidate gives it."""
    merged = []
    for word, section in words:
        parts = [rows[word] for rows in rows_by_candidate.values() if word in rows]
        own_found = any(part["as_written"]["found"] for part in parts)
        own_kinds = {
            k: sum((part["as_written"]["kinds"].get(k) or 0) for part in parts) for k in KINDS
        }
        variants = {}
        names = {name for part in parts for name in part["variants"]}
        for name in names:
            kinds = {}
            for k in KINDS:
                kinds[k] = sum((resolve(part, name)[1].get(k) or 0) for part in parts)
            via = sorted({form for part in parts for form in part["variants"].get(name, {}).get("via", ())})
            flags = [part["variants"][name].get("confirmed") for part in parts if name in part["variants"]]
            confirmed = None if any(flag is None for flag in flags) else any(flags)
            variants[name] = {"via": via, "kinds": kinds, "confirmed": confirmed}
        merged.append(
            {
                "word": word,
                "section": section,
                "as_written": {"found": own_found, "kinds": own_kinds},
                "variants": variants,
            }
        )
    return merged


# --------------------------------------------------------------------------
# The run
# --------------------------------------------------------------------------


def log(message):
    print(message, file=sys.stderr, flush=True)


def run(manifest, exp7_manifest, data, classpath):
    lines = []

    # 1. Target lists, then every file, before anything is parsed.
    lists, list_hashes = {}, {}
    for lang in LANGS:
        entry = manifest["target_lists"][lang]
        list_hashes[lang] = check_list(ROOT / entry["path"], entry.get("sha256"))
    en_entry = exp7_manifest["target_list"]
    list_hashes["en"] = check_list(ROOT / en_entry["path"], en_entry.get("sha256"))
    rows_by_name = {row["name"]: row for row in manifest["candidates"]}
    paths = {}
    for row in manifest["candidates"]:
        paths[row["name"]] = check_file(row, data)
        if "companion" in row:
            check_file(row["companion"], data)
    exp7_rows = {row["name"]: row for row in exp7_manifest["candidates"]}
    exp7_paths = {name: check_file(exp7_rows[name], EXP7 / "data") for name in EN_SET}
    log("hashes: all equal to the manifests")

    for lang in LANGS:
        lists[lang] = read_list(ROOT / manifest["target_lists"][lang]["path"])
    lists["en"] = [(word, section) for word, section in exp7.read_words(ROOT / en_entry["path"])]
    all_words = {lang: [word for word, _ in lists[lang]] for lang in lists}

    # 2. Exception sources.
    cache_kaikki = {}
    cache_dbnary = {}
    pairs_by_source = {}
    skipped = {}
    for lang in LANGS:
        names = [FORM_SOURCES[lang]["languagetool"]] + FORM_SOURCES[lang]["wiktionary"]
        pairs_by_source[lang] = read_form_sources(
            lang, names, rows_by_name, paths, classpath, skipped, cache_kaikki, cache_dbnary
        )

    # 3. Exception variants per language, cut to the keys the lists need.
    variant_forms = {}  # lang -> word -> variant -> forms
    variant_names = {}
    confirm_of = {}  # lang -> word -> forms LanguageTool confirms (None: skipped)
    for lang in LANGS:
        words = sorted({word for key in all_words for word in all_words[key]})
        keys = set(words)
        for word in words:
            keys.update(rules.strip_bases(word, lang))
        tables = {}
        source_names = list(pairs_by_source[lang])
        wiktionary = [name for name in FORM_SOURCES[lang]["wiktionary"] if name in pairs_by_source[lang]]
        groups = [(name, pairs_by_source[lang][name]) for name in source_names]
        groups.append(("wiktionary", set().union(*(pairs_by_source[lang][name] for name in wiktionary))))
        for name, pairs in groups:
            log(f"exceptions {lang}: {name} ({len(pairs)} pairs) ...")
            by_form, stats = table_stats(pairs, lang)
            full, irregular = variants_for(by_form, keys, lang)
            tables[f"full:{name}"] = full
            tables[f"irregular:{name}"] = irregular
            lines.append(
                {
                    "kind": "exceptions",
                    "lang": lang,
                    "source": name,
                    "members": wiktionary if name == "wiktionary" else [name],
                    **stats,
                }
            )
        names = ["as_written", "rules"]
        for name, _ in groups:
            names += [f"rules+irregular:{name}", f"rules+full:{name}"]
        variant_names[lang] = names
        forms_of = {}
        for word in words:
            per = {"rules": rules.reduce_word(word, lang)}
            for name, _ in groups:
                for table in ("irregular", "full"):
                    per[f"rules+{table}:{name}"] = rules.reduce_word(word, lang, tables[f"{table}:{name}"])
            forms_of[word] = per
        variant_forms[lang] = forms_of
        lt_table = tables.get("full:" + FORM_SOURCES[lang]["languagetool"])
        confirm_of[lang] = {word: confirmers(word, lang, lt_table) for word in words}

    # 4. Indexes of the measured candidates, cut to the words and their forms.
    ilis = None
    indexes = {lang: {} for lang in LANGS}
    for lang in LANGS:
        wanted = set()
        for word, per in variant_forms[lang].items():
            wanted.add(word)
            for forms in per.values():
                wanted.update(forms)
        for row in manifest["candidates"]:
            if not sources.measured(row, lang):
                continue
            name = row["name"]
            log(f"index {lang}: {name} ...")
            fmt = row["format"]
            if fmt == "kaikki-jsonl-gz":
                index = cache_kaikki.get((name, lang))
                if index is None:
                    index, _ = sources.kaikki(paths[name], row, lang)
            elif fmt == "wikdict-sqlite-pair":
                index = sources.wikdict_pair(paths[name], row, lang)
            elif fmt == "freedict-tei-tar-xz":
                index = sources.freedict(paths[name], row, lang)
            elif fmt == "dbnary-ttl-bz2":
                index = sources.dbnary(paths[name], row, lang, cache_dbnary)
            elif fmt == "wn-lmf-tar-xz":
                if ilis is None:
                    ilis = sources.oewn_ilis(exp7_paths["oewn-2025"])
                index = sources.omw(paths[name], row, lang, ilis)
            elif fmt == "mythes-dat":
                index = sources.mythes(paths[name], row, lang)
            elif fmt == "apertium-tar":
                index = sources.apertium(paths[name], row, lang)
            else:
                raise Stop(4, f"{name}: no index reader for format {fmt}")
            headwords = len(index)
            index = sources.restrict(index, row, lang)
            indexes[lang][name] = {
                "index": {word: index[word] for word in wanted if word in index},
                "headwords_with_content": len(index),
                "headwords_read": headwords,
            }
    cache_kaikki.clear()

    # 5. EN lookup: EXP-7's set and rules, imported.
    en_wanted = set()
    for lang in LANGS:
        for word in all_words[lang]:
            en_wanted.add(word)
    exceptions_en = None
    en_indexes = {}
    for name in EN_SET:
        row = exp7_rows[name]
        index, _, found = exp7.parse_candidate(row, exp7_paths[name])
        if found is not None:
            exceptions_en = found
        en_indexes[name] = (row, index)
    en_forms = {word: exp7.reduce_word(word, exceptions_en) for word in en_wanted}

    # 6. Lines.
    lines.append(
        {
            "kind": "run",
            "schema": 1,
            "target_lists_sha256": list_hashes,
            "words": {lang: len(lists[lang]) for lang in lists},
            "matching": "exact headword, case included",
            "found": "a headword with at least one non-empty content kind of the candidate",
            "unconfirmed": "found only through forms that are neither a lemma LanguageTool gives for the word, nor a base, nor a lemma it gives for a base",
            "variants": variant_names,
            "skipped": skipped,
            "en_lookup": {"set": list(EN_SET), "rules": "experiments/exp7/coverage.py reduce_word with the OEWN <Form> list"},
        }
    )
    rows_all = {lang: {} for lang in LANGS}  # lang -> list -> candidate -> word -> row
    for lang in LANGS:
        for name, info in indexes[lang].items():
            row = rows_by_name[name]
            provided = set(sources.provides(row, lang))
            lines.append(
                {
                    "kind": "candidate",
                    "candidate": name,
                    "lang": lang,
                    "file": row["file"],
                    "version": row.get("version"),
                    "size": row["size"],
                    "sha256": row["sha256"],
                    "format": row["format"],
                    "provides": [k for k in KINDS if k in provided],
                    "headwords_with_content": info["headwords_with_content"],
                }
            )
            index = info["index"]
            for list_lang in ("es", "ca", "en"):
                per = rows_all[lang].setdefault(list_lang, {}).setdefault(name, {})
                for word, section in lists[list_lang]:
                    result = look_up(word, index, variant_forms[lang][word], provided, confirm_of[lang][word])
                    if list_lang != lang and not result["as_written"]["found"] and not result["variants"]:
                        continue
                    result.update({"kind": "word", "lang": lang, "list": list_lang, "candidate": name, "word": word, "section": section})
                    per[word] = result
                    lines.append(result)
            own = [rows_all[lang][lang][name][word] for word, _ in lists[lang]]
            lines.append(summarize(name, lang, own, variant_names[lang], provided))
        merged = union_rows(rows_all[lang][lang], lang, lists[lang])
        lines.append(summarize("all-candidates", lang, merged, variant_names[lang], set(KINDS), kind="language"))

    # EN rows over the ES and CA lists.
    en_rows = {}
    for list_lang in LANGS:
        for name in EN_SET:
            row, index = en_indexes[name]
            for word, section in lists[list_lang]:
                result = exp7.look_up(word, en_forms[word], index, exp7.PROVIDES[row["format"]])
                if not result["found_with_reduction"]:
                    continue
                entry = {
                    "kind": "word",
                    "lang": "en",
                    "list": list_lang,
                    "candidate": name,
                    "word": word,
                    "section": section,
                    "found_as_written": result["found_as_written"],
                    "found_via": result["found_via"],
                }
                en_rows.setdefault(list_lang, {}).setdefault(word, []).append(entry)
                lines.append(entry)

    lines.extend(false_hits(rows_all, en_rows, lists, variant_names, skipped))
    return lines


def false_hits(rows_all, en_rows, lists, variant_names, skipped, chosen=None):
    """Per pair, the words of one list that another language's lookup (the
    union of its candidates, or its chosen set when `chosen` names one) finds:
    exact headwords, and per variant the words found only through the rules."""
    pairs = [("en", "es"), ("en", "ca"), ("es", "ca"), ("ca", "es"), ("es", "en"), ("ca", "en")]
    out = []
    for lookup, list_lang in pairs:
        words = [word for word, _ in lists[list_lang]]
        if lookup == "en":
            found = en_rows.get(list_lang, {})
            exact = [w for w in words if any(entry["found_as_written"] for entry in found.get(w, ()))]
            rule_only = {"exp7-rules": [w for w in words if w in found and w not in exact]}
            candidates = list(EN_SET)
        else:
            per_candidate = rows_all[lookup].get(list_lang, {})
            if chosen and lookup in chosen:
                per_candidate = {name: rows for name, rows in per_candidate.items() if name in chosen[lookup]}
            candidates = sorted(per_candidate)
            exact = [w for w in words if any(rows.get(w, {}).get("as_written", {}).get("found") for rows in per_candidate.values())]
            rule_only = {}
            for variant in variant_names[lookup][1:]:
                rule_only[variant] = [
                    w
                    for w in words
                    if w not in exact and any(variant in rows.get(w, {}).get("variants", {}) for rows in per_candidate.values())
                ]
        out.append(
            {
                "kind": "false_hits",
                "lookup": lookup,
                "list": list_lang,
                "candidates": candidates,
                "words": len(words),
                "exact": exact,
                "rule_only": rule_only,
            }
        )
    return out


def render(lines) -> str:
    order = {"run": 0, "exceptions": 1, "candidate": 2, "word": 3, "summary": 4, "language": 5, "false_hits": 6}
    lines = sorted(
        lines,
        key=lambda line: (
            order[line["kind"]],
            line.get("lang", line.get("lookup", "")),
            line.get("list", ""),
            line.get("candidate", line.get("source", "")),
            line.get("word", ""),
        ),
    )
    return "".join(json.dumps(line, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n" for line in lines)


def print_summary(lines, out=None):
    out = out or sys.stdout
    run_line = next(line for line in lines if line["kind"] == "run")
    for name, reason in run_line["skipped"].items():
        print(f"SKIPPED {name}: {reason}", file=out)
    for line in lines:
        if line["kind"] == "exceptions":
            print(
                f"exceptions {line['lang']} {line['source']}: full {line['full_pairs']} pairs / {line['full_forms']} forms, "
                f"irregular-only {line['irregular_pairs']} pairs / {line['irregular_forms']} forms",
                file=out,
            )
    for kind in ("summary", "language"):
        for line in lines:
            if line["kind"] != kind:
                continue
            total = line["words"]
            for variant in ("as_written", "rules", *[v for v in line["variants"] if v.startswith("rules+full")]):
                block = line["variants"][variant]
                cells = " ".join(
                    f"{k}={'n/a' if block[k] is None else block[k]}" for k in KINDS
                )
                print(
                    f"{line['lang']} {line['candidate']:<26} {variant:<34} found={block['found']}/{total} "
                    f"unconfirmed={'n/a' if block['unconfirmed'] is None else block['unconfirmed']} {cells}",
                    file=out,
                )
    for line in lines:
        if line["kind"] == "false_hits":
            rule = " ".join(f"{v}={len(w)}" for v, w in line["rule_only"].items())
            print(f"false hits {line['lookup']}<-{line['list']}: exact {len(line['exact'])}/{line['words']}; rule-only {rule}", file=out)


def recompute(results: Path, chosen: dict[str, list[str]]):
    """Coverage and false hits of a chosen set, from results.jsonl."""
    lines = [json.loads(text) for text in results.read_text(encoding="utf-8").splitlines()]
    run_line = next(line for line in lines if line["kind"] == "run")
    provides = {
        (line["lang"], line["candidate"]): set(line["provides"]) for line in lines if line["kind"] == "candidate"
    }
    for lang, names in chosen.items():
        unknown = [name for name in names if (lang, name) not in provides]
        if unknown:
            raise Stop(4, f"--chosen {lang}: not a measured {lang} candidate in {results}: {', '.join(unknown)}")
    rows_all = {lang: {} for lang in LANGS}
    en_rows = {}
    for line in lines:
        if line["kind"] != "word":
            continue
        if line["lang"] == "en":
            en_rows.setdefault(line["list"], {}).setdefault(line["word"], []).append(line)
        else:
            rows_all[line["lang"]].setdefault(line["list"], {}).setdefault(line["candidate"], {})[line["word"]] = line
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    exp7_manifest = json.loads((EXP7 / "candidates.json").read_text(encoding="utf-8"))
    word_lists = {}
    for lang in LANGS:
        entry = manifest["target_lists"][lang]
        check_list(ROOT / entry["path"], entry.get("sha256"))
        word_lists[lang] = read_list(ROOT / entry["path"])
    entry = exp7_manifest["target_list"]
    check_list(ROOT / entry["path"], entry.get("sha256"))
    word_lists["en"] = exp7.read_words(ROOT / entry["path"])
    out = []
    for lang, names in chosen.items():
        per = {name: rows for name, rows in rows_all[lang][lang].items() if name in names}
        merged = union_rows(per, lang, word_lists[lang])
        provided = set().union(*(provides[(lang, name)] for name in names))
        summary = summarize("chosen:" + ",".join(names), lang, merged, run_line["variants"][lang], provided, kind="language")
        out.append(summary)
    out.extend(false_hits(rows_all, en_rows, word_lists, run_line["variants"], run_line["skipped"], chosen))
    return out


# --------------------------------------------------------------------------
# Markdown tables for the record, from results.jsonl alone
# --------------------------------------------------------------------------

KIND_LABELS = {"ru": "RU", "definitions": "Def", "synonyms": "Syn", "en": "EN"}


def _variant_label(variant):
    if variant == "as_written":
        return "as written"
    if variant == "rules" or "+" not in variant:
        return variant
    _, _, rest = variant.partition("+")
    table, _, source = rest.partition(":")
    return f"+{'irr' if table == 'irregular' else 'full'} {source}"


def _cell(block, key):
    value = block[key]
    return "—" if value is None else str(value)


def _names(words):
    return " ".join(words) if words else "—"


def markdown(lines) -> str:
    out = []
    emit = out.append
    run_line = next(line for line in lines if line["kind"] == "run")
    summaries = [line for line in lines if line["kind"] in ("summary", "language")]
    for lang in LANGS:
        variants = run_line["variants"][lang]
        full_variants = [v for v in variants if v in ("as_written", "rules") or v.startswith("rules+full")]
        rows = [line for line in summaries if line["lang"] == lang]
        rows.sort(key=lambda line: (line["kind"] == "language", line["candidate"]))
        total = rows[0]["words"]
        emit(f"\n#### {lang.upper()}: exception lists\n")
        emit("| Source | Files | Full table: pairs / forms | Irregular-only: pairs / forms |")
        emit("|---|---|---|---|")
        for line in lines:
            if line["kind"] == "exceptions" and line["lang"] == lang:
                emit(
                    f"| {line['source']} | {', '.join('`' + m + '`' for m in line['members'])} | "
                    f"{line['full_pairs']} / {line['full_forms']} | {line['irregular_pairs']} / {line['irregular_forms']} |"
                )
        same = all(
            line["variants"][v.replace("+full", "+irregular")] | {"missed": None, "missing_kind": None, "by_section": None}
            == line["variants"][v] | {"missed": None, "missing_kind": None, "by_section": None}
            and line["variants"][v.replace("+full", "+irregular")]["missed"] == line["variants"][v]["missed"]
            for line in rows
            for v in variants
            if v.startswith("rules+full")
        )
        emit(
            f"\nRules + irregular-only list and rules + full table give "
            f"{'the same figures and the same misses for every candidate' if same else 'different figures (see results.jsonl)'}; "
            "the tables below show the full table.\n"
        )
        header = " | ".join(_variant_label(v) for v in full_variants)
        emit(f"\n#### {lang.upper()}: words found (of {total})\n")
        emit(f"| Candidate | Kinds | {header} |")
        emit("|---|---|" + "---|" * len(full_variants))
        for line in rows:
            kinds = ", ".join(KIND_LABELS[k] for k in line["provides"])
            cells = " | ".join(str(line["variants"][v]["found"]) for v in full_variants)
            emit(f"| `{line['candidate']}` | {kinds} | {cells} |")
        for kind in KINDS:
            with_kind = [line for line in rows if kind in line["provides"]]
            if not with_kind:
                continue
            emit(f"\n#### {lang.upper()}: words with {KIND_LABELS[kind]} (of {total})\n")
            emit(f"| Candidate | {header} |")
            emit("|---|" + "---|" * len(full_variants))
            for line in with_kind:
                cells = " | ".join(_cell(line["variants"][v], kind) for v in full_variants)
                emit(f"| `{line['candidate']}` | {cells} |")
        order = ["general", "forms", "irregular", "elision", "middle-dot"]
        sections = sorted(rows[0]["variants"]["as_written"]["by_section"], key=lambda s: (s not in order, order.index(s) if s in order else 0, s))
        lt = next((v for v in variants if v == "rules+full:" + FORM_SOURCES[lang]["languagetool"]), None)
        steps = [v for v in ("as_written", "rules", lt, "rules+full:wiktionary") if v in variants]
        emit(f"\n#### {lang.upper()}: words found per section ({' → '.join(_variant_label(v) for v in steps)})\n")
        by_section = rows[0]["variants"]["as_written"]["by_section"]
        section_header = " | ".join(f"{s} ({by_section[s]['words']})" for s in sections)
        emit("| Candidate | " + section_header + " |")
        emit("|---|" + "---|" * len(sections))
        for line in rows:
            cells = []
            for section in sections:
                parts = [line["variants"][v]["by_section"][section]["found"] for v in steps]
                cells.append(" → ".join(map(str, parts)))
            emit(f"| `{line['candidate']}` | " + " | ".join(cells) + " |")
        union = next(line for line in rows if line["kind"] == "language")
        emit(f"\n#### {lang.upper()}: all candidates together, words with each kind per section\n")
        emit("| Kind | Variant | " + section_header + " |")
        emit("|---|---|" + "---|" * len(sections))
        for kind in KINDS:
            for v in steps:
                cells = " | ".join(_cell(union["variants"][v]["by_section"][s], kind) for s in sections)
                emit(f"| {KIND_LABELS[kind]} | {_variant_label(v)} | {cells} |")
        if lt is not None:
            checked = [v for v in steps if v != "as_written"]
            emit(f"\n#### {lang.upper()}: found only through forms LanguageTool does not confirm\n")
            emit(
                "A rule hit is unconfirmed when none of the forms that found it is a lemma the LanguageTool table "
                "gives for the word, a base of the word, or a lemma it gives for a base. Cells: found (of them unconfirmed).\n"
            )
            emit("| Candidate | " + " | ".join(_variant_label(v) for v in checked) + " |")
            emit("|---|" + "---|" * len(checked))
            for line in rows:
                cells = " | ".join(
                    f"{line['variants'][v]['found']} ({line['variants'][v]['unconfirmed']})" for v in checked
                )
                emit(f"| `{line['candidate']}` | {cells} |")
            for v in checked:
                words = union["variants"][v]["unconfirmed_words"]
                emit(f"- All candidates together, {_variant_label(v)}, unconfirmed ({len(words)}): {_names(words)}")
        emit(f"\n#### {lang.upper()}: misses by name\n")
        for v in full_variants:
            emit(f"- All candidates together, {_variant_label(v)} ({len(union['variants'][v]['missed'])}): {_names(union['variants'][v]['missed'])}")
        for kind in KINDS:
            for v in ("rules", lt):
                if v is None:
                    continue
                missing = union["variants"][v]["missing_kind"][kind]
                emit(f"- No {KIND_LABELS[kind]} in any candidate, {_variant_label(v)} ({len(missing)}): {_names(missing)}")
        for line in rows:
            if line["kind"] == "language":
                continue
            for v in ("rules", lt):
                if v is None:
                    continue
                missed = line["variants"][v]["missed"]
                emit(f"- `{line['candidate']}`, {_variant_label(v)} ({len(missed)}): {_names(missed)}")
    emit("\n#### False hits across languages\n")
    emit("Lookup ← list: the words of the list that the lookup language finds. Exact: a headword as written; rule-only: found only through that variant of the lookup language's rules.\n")
    emit("| Lookup ← list | Exact | " + "Rule-only (variant: count) |")
    emit("|---|---|---|")
    pair_order = [("en", "es"), ("en", "ca"), ("es", "ca"), ("ca", "es"), ("es", "en"), ("ca", "en")]
    hits = sorted(
        (line for line in lines if line["kind"] == "false_hits"),
        key=lambda line: pair_order.index((line["lookup"], line["list"])),
    )
    for line in hits:
        rule = "; ".join(f"{_variant_label(v)}: {len(w)}" for v, w in line["rule_only"].items() if "+irregular" not in v)
        emit(f"| {line['lookup'].upper()} ← {line['list'].upper()} list ({line['words']}) | {len(line['exact'])} | {rule} |")
    for line in hits:
        emit(f"\n**{line['lookup'].upper()} ← {line['list'].upper()} list** ({', '.join('`' + c + '`' for c in line['candidates'])})\n")
        emit(f"- Exact ({len(line['exact'])}): {_names(line['exact'])}")
        for v, words in line["rule_only"].items():
            if "+irregular" in v:
                continue
            emit(f"- Rule-only, {_variant_label(v)} ({len(words)}): {_names(words)}")
    return "\n".join(out) + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", type=Path, default=RECORD / "results.jsonl")
    parser.add_argument("--chosen", action="append", default=[], help="LANG=NAME,NAME (recompute from --out)")
    parser.add_argument("--markdown", action="store_true", help="print the record tables from --out")
    arguments = parser.parse_args(argv)

    if arguments.markdown:
        lines = [json.loads(text) for text in arguments.out.read_text(encoding="utf-8").splitlines()]
        sys.stdout.write(markdown(lines))
        return 0

    if arguments.chosen:
        chosen = {}
        for item in arguments.chosen:
            lang, _, names = item.partition("=")
            if lang not in LANGS or not names:
                print(f"error: bad --chosen {item!r}", file=sys.stderr)
                return 4
            chosen[lang] = names.split(",")
        try:
            for line in recompute(arguments.out, chosen):
                print(json.dumps(line, ensure_ascii=False, sort_keys=True))
        except Stop as stop:
            print(f"error: {stop}", file=sys.stderr)
            return stop.code
        return 0

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        exp7_manifest = json.loads((EXP7 / "candidates.json").read_text(encoding="utf-8"))
        manifest["candidates"], manifest["target_lists"], exp7_manifest["target_list"]
    except (OSError, ValueError, KeyError) as error:
        print(f"error: bad manifest: {error}", file=sys.stderr)
        return 4
    try:
        lines = run(manifest, exp7_manifest, DATA, os.environ.get("MORFOLOGIK_CP"))
    except Stop as stop:
        print(f"error: {stop}", file=sys.stderr)
        return stop.code
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = arguments.out.with_name(arguments.out.name + ".tmp")
    temporary.write_text(render(lines), encoding="utf-8")
    os.replace(temporary, arguments.out)
    print_summary(lines)
    print(f"written: {arguments.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
