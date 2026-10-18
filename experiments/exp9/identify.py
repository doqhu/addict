"""EXP-9a: identify every candidate of experiments/exp9/candidates.json.

For each row: check the file's size and SHA-256 against the manifest (exit 2
on a difference, before anything is parsed), then read the file and count
what its format carries for Spanish (es) and Catalan (ca) -- entries,
headwords and the entries that have each content kind. Nothing is matched
against the target lists here; that is EXP-9b.

Run from the repository root:

    python3 experiments/exp9/identify.py            # every candidate
    python3 experiments/exp9/identify.py NAME ...   # only these rows

The result is written to
_bmad-output/initiative-linux-rust-translator/experiment-exp-9/field-presence.json
(only for a full run) and printed. Files are opened read-only; archives are
read in memory. The morfologik rows alone write the dictionary to a temporary
directory, which is removed afterwards.

Morfologik dictionaries (LanguageTool jars) need Java: set MORFOLOGIK_CP to a
class path holding morfologik-tools 2.1.9 and its dependencies (see the
record); without it those rows report their members only.
"""

import bz2
import collections
import gzip
import hashlib
import io
import json
import lzma
import os
import re
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import xml.etree.ElementTree as ET
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = os.path.join(ROOT, "experiments/exp9/candidates.json")
DATA = os.path.join(ROOT, "experiments/exp9/data")
OUT = os.path.join(
    ROOT,
    "_bmad-output/initiative-linux-rust-translator/experiment-exp-9/field-presence.json",
)
LANGS = ("es", "ca")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 22), b""):
            digest.update(block)
    return digest.hexdigest()


def check_file(entry):
    path = os.path.normpath(os.path.join(DATA, entry["file"]))
    if not os.path.exists(path):
        return path, "missing"
    if os.path.getsize(path) != entry["size"]:
        return path, "size differs"
    if sha256(path) != entry["sha256"]:
        return path, "sha256 differs"
    return path, None


def check(row):
    """The row's file and, when present, its companion (a Hunspell .aff)."""
    path, problem = check_file(row)
    if not problem and "companion" in row:
        _, problem = check_file(row["companion"])
        if problem:
            problem = "companion " + problem
    return path, problem


# --- Kaikki / wiktextract JSONL -------------------------------------------


def kaikki(path, row):
    """Entries of lang_code es/ca. Glosses are in the edition's language."""
    edition = row["edition"]
    needles = [f'"lang_code": "{lang}"'.encode() for lang in LANGS]
    stats = {}
    for lang in LANGS:
        stats[lang] = collections.Counter()
        stats[lang]["_headwords"] = set()
    with gzip.open(path, "rb") as handle:
        for line in handle:
            if not any(needle in line for needle in needles):
                continue
            entry = json.loads(line)
            lang = entry.get("lang_code")
            if lang not in stats:
                continue
            count = stats[lang]
            count["entries"] += 1
            count["_headwords"].add(entry.get("word"))
            senses = entry.get("senses", [])
            glossed = [s for s in senses if s.get("glosses") and "form_of" not in s and "alt_of" not in s]
            if glossed:
                count[f"gloss_{edition}"] += 1
            if any("form_of" in s or "alt_of" in s for s in senses):
                count["form_of"] += 1
            if entry.get("synonyms") or any(s.get("synonyms") for s in senses):
                count["synonyms"] += 1
            if entry.get("forms"):
                count["forms"] += 1
            translations = list(entry.get("translations", []))
            for sense in senses:
                translations.extend(sense.get("translations", []))
            codes = {t.get("lang_code") or t.get("code") for t in translations}
            for code in ("ru", "en", "es", "ca"):
                if code in codes and code != lang:
                    count[f"translation_{code}"] += 1
    return {lang: finish(count) for lang, count in stats.items() if count["entries"]}


def finish(count):
    result = {k: v for k, v in sorted(count.items()) if not k.startswith("_")}
    if "_headwords" in count:
        result["headwords"] = len(count["_headwords"])
    return result


# --- WikDict SQLite -------------------------------------------------------


def connect(path):
    return sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)


def wikdict_pair(path, row):
    db = connect(path)
    rows, heads = db.execute("select count(*), count(distinct written_rep) from translation").fetchone()
    with_sense = db.execute("select count(distinct written_rep) from translation where sense is not null").fetchone()[0]
    result = {"translation_rows": rows, "headwords": heads, "headwords_with_sense_text": with_sense}
    if row.get("reverse"):
        targets = set()
        for (trans,) in db.execute("select trans_list from translation"):
            targets.update(t.strip() for t in (trans or "").split("|") if t.strip())
        result["distinct_target_words"] = len(targets)
    return result


def wikdict_mono(path, row):
    db = connect(path)
    prefix = row["native_prefix"]
    entries, heads = db.execute(
        "select count(*), count(distinct written_rep) from entry where lexentry like ?", (prefix + "/%",)
    ).fetchone()
    forms, form_words = db.execute(
        "select count(*), count(distinct other_written) from form where lexentry like ?", (prefix + "/%",)
    ).fetchone()
    return {"entries": entries, "headwords": heads, "form_rows": forms, "distinct_forms": form_words}


# --- FreeDict TEI ---------------------------------------------------------

TEI = "{http://www.tei-c.org/ns/1.0}"


def freedict(path, row):
    with tarfile.open(path) as archive:
        data = archive.extractfile(row["members"]["tei"]).read()
    count = collections.Counter()
    heads = set()
    for _, element in ET.iterparse(io.BytesIO(data)):
        if element.tag != TEI + "entry":
            continue
        count["entries"] += 1
        for orth in element.iter(TEI + "orth"):
            heads.add(orth.text)
        if any(c.get("type") == "trans" for c in element.iter(TEI + "cit")):
            count["with_translation"] += 1
        if any(True for _ in element.iter(TEI + "def")):
            count["with_def"] += 1
        if any(x.get("type") == "syn" for x in element.iter(TEI + "xr")):
            count["with_synonym_xr"] += 1
        element.clear()
    result = dict(count)
    result["headwords"] = len(heads)
    return result


# --- Open Multilingual Wordnet (WN-LMF) ------------------------------------


def omw(path, row):
    with tarfile.open(path) as archive:
        data = archive.extractfile(row["members"]["xml"]).read()
    count = collections.Counter()
    lemmas = set()
    for _, element in ET.iterparse(io.BytesIO(data)):
        if element.tag == "Lemma":
            lemmas.add(element.get("writtenForm"))
        elif element.tag == "LexicalEntry":
            count["entries"] += 1
            if element.find("Form") is not None:
                count["with_form"] += 1
            element.clear()
        elif element.tag == "Synset":
            count["synsets"] += 1
            if element.find("Definition") is not None:
                count["synsets_with_definition"] += 1
            if len((element.get("members") or "").split()) > 1:
                count["synsets_with_2plus_members"] += 1
            if element.get("ili") and element.get("ili") != "in":
                count["synsets_with_ili"] += 1
            element.clear()
    result = dict(count)
    result["headwords"] = len(lemmas)
    return result


# --- DBnary Turtle --------------------------------------------------------

DBNARY_PATTERNS = {
    "lexical_entries": re.compile(r"\bontolex:LexicalEntry\b"),
    "definitions": re.compile(r"\bskos:definition\b"),
    "synonym_relations": re.compile(r"\bdbnary:synonym\b"),
    "translations": re.compile(r"\bdbnary:Translation\b"),
    "translations_to_rus": re.compile(r"dbnary:targetLanguage\s+lexvo:rus\b"),
    "translations_to_eng": re.compile(r"dbnary:targetLanguage\s+lexvo:eng\b"),
    "translations_to_spa": re.compile(r"dbnary:targetLanguage\s+lexvo:spa\b"),
    "translations_to_cat": re.compile(r"dbnary:targetLanguage\s+lexvo:cat\b"),
    "other_forms": re.compile(r"\bontolex:otherForm\b"),
}


def dbnary(path, row):
    count = collections.Counter()
    with bz2.open(path, "rt", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            for key, pattern in DBNARY_PATTERNS.items():
                count[key] += len(pattern.findall(line))
    return {"occurrences": dict(count), "note": "occurrence counts of Turtle terms, not distinct words"}


# --- MyThes thesaurus -----------------------------------------------------


def mythes(path, row):
    with open(path, "rb") as handle:
        encoding = handle.readline().decode("ascii").strip()
        text = handle.read().decode(encoding.lower().replace("iso8859", "iso8859-"), errors="replace")
    entries = 0
    meanings = 0
    for line in text.split("\n"):
        if re.match(r"^[^|(\-][^|]*\|\d+$", line):
            entries += 1
        elif line.startswith(("-|", "(")):
            meanings += 1
    return {"encoding": encoding, "headwords": entries, "meaning_lines": meanings}


# --- Hunspell -------------------------------------------------------------


def hunspell(path, row):
    aff = os.path.normpath(os.path.join(DATA, row["members"]["aff"]))
    with open(path, "rb") as handle:
        declared = int(handle.readline().strip())
    rules = 0
    with open(aff, "rb") as handle:
        for line in handle:
            if re.match(rb"^(SFX|PFX) \S+ \S+ \S+ \S+", line):
                rules += 1
    return {"stems_declared": declared, "affix_rules": rules, "note": "stems and affix rules: forms are generated, not listed"}


def hunspell_zip(path, row):
    with zipfile.ZipFile(path) as archive:
        dic = archive.read(row["members"]["dic"])
        aff = archive.read(row["members"]["aff"])
    rules = sum(1 for line in aff.split(b"\n") if re.match(rb"^(SFX|PFX) \S+ \S+ \S+ \S+", line))
    return {"stems_declared": int(dic.split(b"\n", 1)[0]), "affix_rules": rules, "members": sorted(row["members"].values())}


# --- LanguageTool morfologik ----------------------------------------------


def morfologik(path, row):
    with zipfile.ZipFile(path) as archive:
        members = {i.filename: i.file_size for i in archive.infolist() if not i.is_dir()}
        dictionary = archive.read(row["members"]["dict"])
        info = archive.read(row["members"]["info"])
    result = {"members": {k: v for k, v in members.items() if k.startswith("org/")}}
    classpath = os.environ.get("MORFOLOGIK_CP")
    if not classpath:
        result["note"] = "MORFOLOGIK_CP not set: not decompiled"
        return result
    with tempfile.TemporaryDirectory() as scratch:
        name = os.path.basename(row["members"]["dict"])
        with open(os.path.join(scratch, name), "wb") as handle:
            handle.write(dictionary)
        with open(os.path.join(scratch, name[:-5] + ".info"), "wb") as handle:
            handle.write(info)
        out = os.path.join(scratch, "out.txt")
        subprocess.run(
            ["java", "-cp", classpath, "morfologik.tools.Launcher", "dict_decompile",
             "-i", os.path.join(scratch, name), "-o", out],
            check=True, capture_output=True,
        )
        lemmas, forms, lines = set(), set(), 0
        with open(out, encoding="utf-8") as handle:
            for line in handle:
                parts = line.rstrip("\n").split("_")
                if len(parts) < 3:
                    continue
                lines += 1
                lemmas.add(parts[0])
                forms.add(parts[1])
    result.update({"lines": lines, "distinct_lemmas": len(lemmas), "distinct_forms": len(forms),
                   "layout": "lemma_form_tag"})
    return result


# --- Apertium ---------------------------------------------------------------


def apertium(path, row):
    result = {}
    with tarfile.open(path) as archive:
        for member in row["members"].values():
            data = archive.extractfile(member).read()
            result[member] = {"e_elements": len(re.findall(rb"<e[ >]", data)),
                              "pardef_elements": len(re.findall(rb"<pardef[ >]", data))}
    return result


# --- spanish-dict-tools source ---------------------------------------------


def source_tree(path, row):
    result = {}
    with tarfile.open(path) as archive:
        for member in archive.getmembers():
            name = member.name.split("/", 1)[-1]
            if name in row["members"].values():
                data = archive.extractfile(member).read()
                result[name] = {"bytes": member.size, "lines": data.count(b"\n")}
            elif name.startswith("diccionari-arrel/models-verbals/") and member.isfile():
                result.setdefault("verb_models", 0)
                result["verb_models"] += 1
    return result


PROFILERS = {
    "kaikki-jsonl-gz": kaikki,
    "wikdict-sqlite-pair": wikdict_pair,
    "wikdict-sqlite-mono": wikdict_mono,
    "freedict-tei-tar-xz": freedict,
    "wn-lmf-tar-xz": omw,
    "dbnary-ttl-bz2": dbnary,
    "mythes-dat": mythes,
    "hunspell-aff-dic": hunspell,
    "hunspell-zip": hunspell_zip,
    "morfologik-jar": morfologik,
    "apertium-tar": apertium,
    "source-tar-gz": source_tree,
}


def main(names):
    with open(MANIFEST, encoding="utf-8") as handle:
        manifest = json.load(handle)
    rows = [r for r in manifest["candidates"] if not names or r["name"] in names]
    paths = {}
    for row in rows:
        path, problem = check(row)
        if problem:
            print(f"{row['name']}: {problem}", file=sys.stderr)
            return 2
        paths[row["name"]] = path
    results = {}
    for row in rows:
        path = paths[row["name"]]
        print(f"{row['name']} ...", file=sys.stderr, flush=True)
        results[row["name"]] = PROFILERS[row["format"]](path, row)
        print(json.dumps({row["name"]: results[row["name"]]}, ensure_ascii=False))
    if not names:
        with open(OUT, "w", encoding="utf-8") as handle:
            json.dump(results, handle, ensure_ascii=False, indent=1, sort_keys=True)
            handle.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
