"""EXP-9b: readers that turn a candidate file of experiments/exp9/candidates.json
into a headword index for Spanish (es) or Catalan (ca), and the form sources
into form -> lemma pairs.

An index maps a headword, exactly as the file writes it, to the number of
items of each content kind it carries:

- `ru`: Russian translations or glosses;
- `definitions`: monolingual definitions (Spanish for es, Catalan for ca);
- `synonyms`: synonyms in the same language;
- `en`: English glosses or translations.

Only headwords with at least one non-zero kind are kept: an entry that only
points at another word (a Kaikki form-of entry) carries no article. Counts are
for information; a kind counts for a word when it is non-zero. Reverse files
(keyed by Russian or English) are inverted: each target word becomes a
headword, counted once per source entry.

`identify.py` profiles the same files; its output is not changed by this
module, which reads them again with the headword kept.
"""

from __future__ import annotations

import bz2
import collections
import gzip
import io
import json
import os
import re
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile
import xml.etree.ElementTree as ET
import zipfile

KINDS = ("ru", "definitions", "synonyms", "en")

# Formats whose forms come only from a generator (Hunspell affixes,
# lttoolbox paradigms, the Perl scripts of spanish-dict-tools): not measured.
GENERATOR_ONLY = {"hunspell-aff-dic", "hunspell-zip", "source-tar-gz"}


def _index():
    return collections.defaultdict(collections.Counter)


def _finish(index) -> dict[str, dict[str, int]]:
    return {
        word: {kind: counts[kind] for kind in KINDS if counts[kind]}
        for word, counts in index.items()
        if any(counts[kind] for kind in KINDS)
    }


def _content_kinds(row) -> list[str]:
    return [kind for kind in row.get("kinds", ()) if kind in KINDS]


def provides(row, lang: str) -> list[str]:
    """The content kinds the row gives for `lang`. A Kaikki edition gives
    definitions only for its own language: the Catalan entries of Spanish
    Wiktionary have Spanish glosses, which are no kind of a Catalan article."""
    kinds = _content_kinds(row)
    if row["format"] == "kaikki-jsonl-gz" and gloss_kind(row["edition"], lang) != "definitions":
        kinds = [kind for kind in kinds if kind != "definitions"]
    return kinds


def measured(row, lang: str) -> bool:
    """Whether the row is an index candidate of `lang`: it carries a content
    kind for that language and its entries can be read without a generator."""
    if lang not in row.get("languages", ()) or row["format"] in GENERATOR_ONLY:
        return False
    if row["format"] == "apertium-tar":
        return "bidix" in row.get("members", {})
    return bool(_content_kinds(row))


def restrict(index, row, lang):
    """Only the kinds the row gives for `lang`; a headword left with none
    is dropped. A kind outside them is "not in the format" (null), never 0."""
    kinds = provides(row, lang)
    result = {}
    for word, counts in index.items():
        kept = {kind: count for kind, count in counts.items() if kind in kinds}
        if kept:
            result[word] = kept
    return result


# --------------------------------------------------------------------------
# Kaikki / wiktextract JSONL
# --------------------------------------------------------------------------

FORM_TAGS_SKIPPED = {"table-tags", "inflection-template", "class", "romanization", "canonical"}


def _open_lines(path):
    """Decompressed lines of a .gz file, through pigz when it is installed."""
    pigz = shutil.which("pigz")
    if pigz:
        process = subprocess.Popen([pigz, "-dc", path], stdout=subprocess.PIPE, bufsize=1 << 20)
        try:
            yield from process.stdout
        finally:
            process.stdout.close()
            if process.wait() != 0:
                raise OSError(f"pigz failed on {path}")
    else:
        with gzip.open(path, "rb") as handle:
            yield from handle


def gloss_kind(edition: str, lang: str):
    """What a gloss of an entry of `lang` is in this edition."""
    if edition == lang:
        return "definitions"
    return {"en": "en", "ru": "ru"}.get(edition)


def kaikki(path, row, lang, forms=False):
    """(index, pairs). `pairs` (form, lemma) are collected when `forms`: the
    words a form-of or alt-of sense points at, and the `forms` list of a
    lemma entry without the table tags and the forms that are not one word."""
    from rules import is_word  # noqa: PLC0415

    kind_of_gloss = gloss_kind(row["edition"], lang)
    needle = f'"lang_code": "{lang}"'.encode()
    index = _index()
    pairs: set[tuple[str, str]] = set()
    for line in _open_lines(path):
        if needle not in line:
            continue
        entry = json.loads(line)
        if entry.get("lang_code") != lang:
            continue
        word = entry.get("word")
        if not isinstance(word, str):
            continue
        senses = entry.get("senses") or []
        counts = index[word]
        if kind_of_gloss:
            counts[kind_of_gloss] += sum(
                1
                for sense in senses
                if sense.get("glosses") and "form_of" not in sense and "alt_of" not in sense
            )
        synonyms = set()
        translations = collections.defaultdict(set)
        for holder in [entry] + senses:
            for synonym in holder.get("synonyms") or ():
                if synonym.get("word") and synonym["word"] != word:
                    synonyms.add(synonym["word"])
            for translation in holder.get("translations") or ():
                code = translation.get("lang_code") or translation.get("code")
                if code in ("ru", "en") and code != row["edition"] and translation.get("word"):
                    translations[code].add(translation["word"])
        counts["synonyms"] += len(synonyms)
        for code, words in translations.items():
            counts[code] += len(words)
        if forms:
            for sense in senses:
                for key in ("form_of", "alt_of"):
                    for target in sense.get(key) or ():
                        lemma = target.get("word")
                        if isinstance(lemma, str) and lemma != word and is_word(word) and is_word(lemma):
                            pairs.add((word, lemma))
            for form in entry.get("forms") or ():
                text = form.get("form")
                if (
                    isinstance(text, str)
                    and text != word
                    and not FORM_TAGS_SKIPPED & set(form.get("tags") or ())
                    and is_word(text)
                    and is_word(word)
                ):
                    pairs.add((text, word))
    return _finish(index), pairs


# --------------------------------------------------------------------------
# WikDict SQLite
# --------------------------------------------------------------------------


def _connect(path):
    return sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)


def _split_list(text):
    return [part.strip() for part in (text or "").split("|") if part.strip()]


def wikdict_pair(path, row, lang):
    (kind,) = _content_kinds(row)
    database = _connect(path)
    targets = collections.defaultdict(set)
    for written, trans_list in database.execute("select written_rep, trans_list from translation"):
        if row.get("reverse"):
            for target in _split_list(trans_list):
                targets[target].add(written)
        elif written:
            targets[written].update(_split_list(trans_list))
    database.close()
    index = _index()
    for word, items in targets.items():
        index[word][kind] = len(items)
    return _finish(index)


def wikdict_forms(path, row, lang):
    from rules import is_word  # noqa: PLC0415

    database = _connect(path)
    prefix = row["native_prefix"] + "/%"
    pairs = set()
    for form, lemma in database.execute(
        "select f.other_written, e.written_rep from form f join entry e on e.lexentry = f.lexentry "
        "where f.lexentry like ?",
        (prefix,),
    ):
        if form and lemma and form != lemma and is_word(form) and is_word(lemma):
            pairs.add((form, lemma))
    database.close()
    return pairs


# --------------------------------------------------------------------------
# FreeDict TEI
# --------------------------------------------------------------------------

TEI = "{http://www.tei-c.org/ns/1.0}"


def freedict(path, row, lang):
    (kind,) = _content_kinds(row)
    with tarfile.open(path) as archive:
        data = archive.extractfile(row["members"]["tei"]).read()
    targets = collections.defaultdict(set)
    for _, element in ET.iterparse(io.BytesIO(data)):
        if element.tag != TEI + "entry":
            continue
        orths = [(orth.text or "").strip() for orth in element.iter(TEI + "orth")]
        quotes = set()
        for cit in element.iter(TEI + "cit"):
            if cit.get("type") == "trans":
                for quote in cit.iter(TEI + "quote"):
                    if (quote.text or "").strip():
                        quotes.add(quote.text.strip())
        for orth in filter(None, orths):
            if row["name"] in REVERSE_FREEDICT:
                for quote in quotes:
                    targets[quote].add(orth)
            else:
                targets[orth].update(quotes)
        element.clear()
    index = _index()
    for word, items in targets.items():
        index[word][kind] = len(items)
    return _finish(index)


REVERSE_FREEDICT = {"freedict-rus-spa-tei", "freedict-eng-spa-tei", "freedict-eng-cat-tei"}


# --------------------------------------------------------------------------
# DBnary Turtle (pretty-printed: one block per subject, blocks separated by
# an empty line, one predicate per line)
# --------------------------------------------------------------------------

LITERAL = r'"((?:[^"\\]|\\.)*)"'
ESCAPES = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\", "'": "'"}


def unescape(text: str) -> str:
    def replace(match):
        body = match.group(1)
        if body[0] in "uU":
            return chr(int(body[1:], 16))
        return ESCAPES.get(body, body)

    return re.sub(r"\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|.)", replace, text)


def _blocks(path):
    """Each block of the file as a list of lines, through lbzip2 when
    installed."""
    tool = shutil.which("lbzip2") or shutil.which("pbzip2")
    if tool:
        process = subprocess.Popen([tool, "-dc", path], stdout=subprocess.PIPE, bufsize=1 << 20)
        stream = io.TextIOWrapper(process.stdout, encoding="utf-8", errors="replace")
    else:
        process = None
        stream = bz2.open(path, "rt", encoding="utf-8", errors="replace")
    block: list[str] = []
    try:
        for line in stream:
            if line.strip():
                block.append(line)
            elif block:
                yield block
                block = []
        if block:
            yield block
    finally:
        stream.close()
        if process is not None and process.wait() != 0:
            raise OSError(f"{tool} failed on {path}")


def _objects(block_text: str, predicate: str) -> list[str]:
    """The objects of `predicate` in a block, as written (names or literals)."""
    found = []
    for match in re.finditer(rf"(?:^|\s){re.escape(predicate)}\s+(.+?)\s*[;.]?\s*$", block_text, re.M):
        found.extend(part.strip() for part in re.split(r"\s,\s", match.group(1)))
    return found


def _literal(block_text: str, predicate: str):
    match = re.search(rf"{re.escape(predicate)}\s+{LITERAL}", block_text)
    return unescape(match.group(1)) if match else None


DBNARY_LANG = {"es": "spa", "ca": "cat"}


def dbnary_entries(path, lang):
    """Parse a DBnary ontolex file of the language's own edition. Returns
    (labels: entry id -> headword, per-entry kind sets)."""
    code = DBNARY_LANG[lang]
    labels: dict[str, str] = {}
    sense_owner: dict[str, str] = {}
    page_entries: dict[str, list[str]] = {}
    definitions = collections.Counter()
    synonyms = collections.defaultdict(set)
    translations = []  # (targets, kind, translation id)
    for block in _blocks(path):
        text = "".join(block)
        subject = text.split(None, 1)[0]
        if "ontolex:LexicalEntry" in text and f"lexvo:{code}" in text:
            label = _literal(text, "rdfs:label")
            if label is not None:
                labels[subject] = label
            for sense in _objects(text, "ontolex:sense"):
                sense_owner[sense] = subject
        elif "dbnary:Page" in text:
            page_entries[subject] = _objects(text, "dbnary:describes")
        if "dbnary:Translation" in text:
            language = re.search(r"dbnary:targetLanguage\s+lexvo:(\w+)", text)
            kind = {"rus": "ru", "eng": "en"}.get(language.group(1)) if language else None
            if kind:
                translations.append((_objects(text, "dbnary:isTranslationOf"), kind, subject))
            continue
        definitions[subject] += text.count("skos:definition")
        for synonym in _objects(text, "dbnary:synonym"):
            synonyms[subject].add(synonym)

    def owners(name):
        if name in labels:
            return [name]
        if name in sense_owner:
            return [sense_owner[name]]
        return [entry for entry in page_entries.get(name, ()) if entry in labels]

    kinds = collections.defaultdict(lambda: collections.defaultdict(set))
    for subject, count in definitions.items():
        for entry in owners(subject):
            kinds[entry]["definitions"].update(f"{subject}#{n}" for n in range(count))
    for subject, items in synonyms.items():
        for entry in owners(subject):
            kinds[entry]["synonyms"].update(items)
    for targets, kind, translation in translations:
        for entry in {owner for target in targets for owner in owners(target)}:
            kinds[entry][kind].add(translation)
    return labels, kinds


def dbnary(path, row, lang, cache=None):
    if row["name"] == "dbnary-ru-ontolex":
        return dbnary_reverse(path, lang)
    labels, kinds = (cache or {}).get(row["name"]) or dbnary_entries(path, lang)
    if cache is not None:
        cache[row["name"]] = (labels, kinds)
    index = _index()
    for entry, items in kinds.items():
        for kind, values in items.items():
            index[labels[entry]][kind] += len(values)
    return _finish(index)


def dbnary_reverse(path, lang):
    """Russian entries of Russian Wiktionary: each translation into the
    language gives its written form as a headword with one Russian item."""
    code = DBNARY_LANG[lang]
    needle = f"lexvo:{code}"
    targets = collections.defaultdict(set)
    for block in _blocks(path):
        text = "".join(block)
        if needle not in text or "dbnary:Translation" not in text:
            continue
        if not re.search(rf"dbnary:targetLanguage\s+lexvo:{code}\b", text):
            continue
        form = _literal(text, "dbnary:writtenForm")
        if form:
            sources = _objects(text, "dbnary:isTranslationOf")
            targets[form.strip()].add(sources[0] if sources else text.split(None, 1)[0])
    index = _index()
    for word, items in targets.items():
        index[word]["ru"] = len(items)
    return _finish(index)


def dbnary_forms(path, labels):
    """Pairs of the Catalan morphology file. A form under an entry gives
    (form, entry headword); an infinitive form under an entry whose headword
    differs means the entry is itself a form: (headword, infinitive)."""
    from rules import is_word  # noqa: PLC0415

    other_forms: dict[str, list[str]] = {}
    written: dict[str, tuple[str, bool]] = {}
    for block in _blocks(path):
        text = "".join(block)
        subject = text.split(None, 1)[0]
        if "ontolex:otherForm" in text:
            other_forms.setdefault(subject, []).extend(_objects(text, "ontolex:otherForm"))
        if "ontolex:writtenRep" in text:
            rep = _literal(text, "ontolex:writtenRep")
            if rep is not None:
                written[subject] = (rep, "lexinfo:infinitive" in text)
    pairs = set()
    for entry, forms in other_forms.items():
        lemma = labels.get(entry)
        if lemma is None:
            continue
        for form in forms:
            if form not in written:
                continue
            rep, infinitive = written[form]
            pair = (lemma, rep) if infinitive else (rep, lemma)
            if pair[0] != pair[1] and is_word(pair[0]) and is_word(pair[1]):
                pairs.add(pair)
    return pairs


# --------------------------------------------------------------------------
# Open Multilingual Wordnet (WN-LMF)
# --------------------------------------------------------------------------


def oewn_ilis(path) -> set[str]:
    """The ILI ids of the synsets of Open English WordNet (EXP-7's file)."""
    ilis = set()
    with gzip.open(path, "rb") as handle:
        for _, element in ET.iterparse(handle):
            if element.tag.rsplit("}", 1)[-1] == "Synset":
                if element.get("ili") and element.get("ili") != "in":
                    ilis.add(element.get("ili"))
                element.clear()
            elif element.tag.rsplit("}", 1)[-1] == "LexicalEntry":
                element.clear()
    return ilis


def omw(path, row, lang, ilis):
    """Definitions: senses whose synset has a definition. Synonyms: the other
    members of the synsets. English: senses whose synset's ILI is a synset of
    OEWN 2025 (its English lemmas and definition give the gloss)."""
    with tarfile.open(path) as archive:
        data = archive.extractfile(row["members"]["xml"]).read()
    entries = []  # (lemma, synsets)
    lemma_of = {}
    synsets = {}
    for _, element in ET.iterparse(io.BytesIO(data)):
        if element.tag == "LexicalEntry":
            lemma = element.find("Lemma").get("writtenForm")
            targets = [sense.get("synset") for sense in element.findall("Sense")]
            entries.append((lemma, targets))
            # Synset members name senses in OMW 2.0 (entries in older WN-LMF).
            lemma_of[element.get("id")] = lemma
            for sense in element.findall("Sense"):
                lemma_of[sense.get("id")] = lemma
            element.clear()
        elif element.tag == "Synset":
            synsets[element.get("id")] = (
                element.find("Definition") is not None and bool((element.find("Definition").text or "").strip()),
                (element.get("members") or "").split(),
                element.get("ili") in ilis,
            )
            element.clear()
    index = _index()
    synonym_sets = collections.defaultdict(set)
    for lemma, targets in entries:
        for target in targets:
            definition, members, english = synsets.get(target, (False, [], False))
            index[lemma]["definitions"] += definition
            index[lemma]["en"] += english
            for member in members:
                other = lemma_of.get(member)
                if other and other != lemma:
                    synonym_sets[lemma].add(other)
    for lemma, others in synonym_sets.items():
        index[lemma]["synonyms"] = len(others)
    return _finish(index)


# --------------------------------------------------------------------------
# MyThes thesaurus
# --------------------------------------------------------------------------


def mythes(path, row, lang):
    with open(path, "rb") as handle:
        encoding = handle.readline().decode("ascii").strip()
        text = handle.read().decode(encoding.lower().replace("iso8859", "iso8859-"), errors="replace")
    lines = text.split("\n")
    index = _index()
    at = 0
    while at < len(lines):
        match = re.match(r"^([^|]+)\|(\d+)$", lines[at])
        at += 1
        if not match:
            continue
        word, count = match.group(1), int(match.group(2))
        synonyms = set()
        for line in lines[at : at + count]:
            synonyms.update(part.strip() for part in line.split("|")[1:] if part.strip())
        at += count
        synonyms.discard(word)
        index[word]["synonyms"] += len(synonyms)
    return _finish(index)


# --------------------------------------------------------------------------
# Apertium bilingual dictionary (eng-X: <l> English, <r> the other side)
# --------------------------------------------------------------------------


def _side(xml: str) -> str | None:
    """The lemma of one side: its text before the first tag symbol, or None
    for a multi-word lemma."""
    if "<b/>" in xml or "<g>" in xml:
        return None
    text = re.split(r"<s ", xml, maxsplit=1)[0]
    text = re.sub(r"<[^>]+>", "", text).strip()
    return text or None


def apertium(path, row, lang):
    with tarfile.open(path) as archive:
        data = archive.extractfile(row["members"]["bidix"]).read().decode("utf-8")
    targets = collections.defaultdict(set)
    for entry in re.finditer(r"<e[ >].*?</e>", data, re.S):
        body = entry.group(0)
        pair = re.search(r"<l>(.*?)</l>\s*<r>(.*?)</r>", body, re.S)
        if pair:
            english, other = _side(pair.group(1)), _side(pair.group(2))
        else:
            same = re.search(r"<i>(.*?)</i>", body, re.S)
            english = other = _side(same.group(1)) if same else None
        if english and other:
            targets[other].add(english)
    index = _index()
    for word, items in targets.items():
        index[word]["en"] = len(items)
    return _finish(index)


# --------------------------------------------------------------------------
# LanguageTool morfologik dictionaries
# --------------------------------------------------------------------------


def morfologik_forms(path, row, classpath):
    """Pairs (form, lemma) of a LanguageTool POS dictionary, decompiled with
    morfologik-tools into lemma_form_tag lines in a temporary directory."""
    from rules import is_word  # noqa: PLC0415

    with zipfile.ZipFile(path) as archive:
        dictionary = archive.read(row["members"]["dict"])
        info = archive.read(row["members"]["info"])
    pairs = set()
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
        with open(out, encoding="utf-8") as handle:
            for line in handle:
                parts = line.rstrip("\n").split("_")
                if len(parts) < 3:
                    continue
                lemma, form = parts[0], parts[1]
                if form != lemma and is_word(form) and is_word(lemma):
                    pairs.add((form, lemma))
    return pairs
