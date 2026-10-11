"""Tests of coverage.py on small inline fixtures: the reduction rules, each
parser, and every row of the I/O matrix of the EXP-7 plan."""

import base64
import contextlib
import gzip
import hashlib
import io
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import coverage  # noqa: E402

WN_LMF = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE LexicalResource SYSTEM "http://globalwordnet.github.io/schemas/WN-LMF-1.3.dtd">
<LexicalResource xmlns:dc="https://globalwordnet.github.io/schemas/dc/">
  <Lexicon id="oewn" label="t" language="en" email="t" license="t" version="t">
    <LexicalEntry id="e-bank-n">
      <Lemma writtenForm="bank" partOfSpeech="n"/>
      <Sense id="s1" synset="ss-1"/>
      <Sense id="s2" synset="ss-2"/>
    </LexicalEntry>
    <LexicalEntry id="e-shore-n">
      <Lemma writtenForm="shore" partOfSpeech="n"/>
      <Sense id="s3" synset="ss-1"/>
    </LexicalEntry>
    <LexicalEntry id="e-go-v">
      <Lemma writtenForm="go" partOfSpeech="v"/>
      <Form writtenForm="went"/>
      <Form writtenForm="gone"/>
      <Sense id="s4" synset="ss-3"/>
    </LexicalEntry>
    <LexicalEntry id="e-lonely-a">
      <Lemma writtenForm="lonely" partOfSpeech="a"/>
      <Sense id="s5" synset="ss-4"/>
    </LexicalEntry>
    <Synset id="ss-1" members="e-bank-n e-shore-n" partOfSpeech="n">
      <Definition>sloping land beside water</Definition>
    </Synset>
    <Synset id="ss-2" members="e-bank-n" partOfSpeech="n">
      <Definition>a financial institution</Definition>
    </Synset>
    <Synset id="ss-3" members="e-go-v" partOfSpeech="v">
      <Definition>change location</Definition>
    </Synset>
    <Synset id="ss-4" members="e-lonely-a" partOfSpeech="a">
      <Definition>lacking companions</Definition>
    </Synset>
  </Lexicon>
</LexicalResource>
"""

TEI = """<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <teiHeader xml:lang="en"><fileDesc><titleStmt><title>t</title></titleStmt></fileDesc></teiHeader>
  <text><body xml:lang="en">
    <entry>
      <form><orth>bank</orth><pron>/b/</pron></form>
      <gramGrp><pos>n</pos></gramGrp>
      <sense>
        <cit type="trans" xml:lang="ru"><quote>берег</quote><quote>брег</quote></cit>
        <sense><def>An edge of a river.</def></sense>
      </sense>
      <sense>
        <cit type="trans" xml:lang="ru"><quote>банк</quote></cit>
        <sense><def>An institution.</def></sense>
        <sense><def>A branch office.</def></sense>
      </sense>
    </entry>
    <entry>
      <form><orth>bank</orth></form>
      <gramGrp><pos>v</pos></gramGrp>
      <sense><cit type="trans" xml:lang="ru"><quote>накреняться</quote></cit></sense>
    </entry>
    <entry>
      <form><orth>go</orth></form>
      <sense><cit type="trans" xml:lang="de"><quote>gehen</quote></cit></sense>
    </entry>
  </body></text>
</TEI>
"""

KAIKKI = [
    {
        "word": "bank",
        "lang_code": "en",
        "pos": "noun",
        "senses": [
            {"glosses": ["An institution."], "synonyms": [{"word": "lender"}]},
            {
                "glosses": ["Edge of a river."],
                "translations": [{"lang_code": "ru", "word": "берег"}],
            },
        ],
        "synonyms": [{"word": "shore"}, {"word": "bank"}],
        "translations": [
            {"code": "ru", "word": "банк"},
            {"lang_code": "ru", "word": "берег"},
            {"lang_code": "de", "word": "Bank"},
        ],
    },
    {
        "word": "banks",
        "lang_code": "en",
        "pos": "noun",
        "senses": [{"glosses": ["plural of bank"], "form_of": [{"word": "bank"}]}],
    },
    {
        "word": "color",
        "lang_code": "en",
        "pos": "noun",
        "senses": [{"glosses": ["Alternative form of colour"], "alt_of": [{"word": "colour"}]}],
    },
    {"word": "bank", "lang_code": "de", "pos": "noun", "senses": [{"glosses": ["bench"]}]},
    {"title": "Bank", "redirect": "bank", "lang_code": "en"},
]


def dictd_number(value: int) -> str:
    digits = ""
    while True:
        digits = coverage.DICTD_ALPHABET[value % 64] + digits
        value //= 64
        if not value:
            return digits


def dictd_files(articles):
    """(index bytes, dict bytes) of a dictd database of (headword, text)."""
    body = b"00-database-info\nt\n"
    index = ["00databaseinfo\t" + dictd_number(0) + "\t" + dictd_number(len(body))]
    for headword, text in articles:
        data = text.encode()
        index.append(f"{headword}\t{dictd_number(len(body))}\t{dictd_number(len(data))}")
        body += data
    return ("\n".join(index) + "\n").encode(), body


DICTD = [
    ("bank", "bank /b/ <n>\n1. бе́рег, брег\nAn edge of a river.\n2. банк 2.\nA branch.\n 3.\nA sum.\n"),
    ("bank", "bank /b/ <v>\n1. накреня́ться\nTo roll.\n"),
    ("set off", "set off <v>\nотпра́виться\nTo leave.\n"),
    ("nodef", "nodef <n>\nслово\n"),
]


def tar_xz(members: dict) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:xz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def kaikki_gz(records=KAIKKI) -> bytes:
    return gzip.compress("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in records).encode())


class Reduction(unittest.TestCase):
    def test_suffix_rules(self):
        for word, form in [
            ("banks", "bank"),
            ("boxes", "box"),
            ("matches", "match"),
            ("wishes", "wish"),
            ("classes", "class"),
            ("cities", "city"),
            ("issues", "issue"),
            ("heroes", "hero"),
            ("women", "woman"),
            ("walked", "walk"),
            ("moved", "move"),
            ("tried", "try"),
            ("stopped", "stop"),
            ("walking", "walk"),
            ("making", "make"),
            ("running", "run"),
            ("lying", "lie"),
            ("faster", "fast"),
            ("larger", "large"),
            ("happier", "happy"),
            ("happiest", "happy"),
            ("biggest", "big"),
            ("nicest", "nice"),
        ]:
            self.assertIn(form, coverage.suffix_forms(word), word)

    def test_rule_order_is_fixed_and_forms_are_unique(self):
        self.assertEqual(coverage.suffix_forms("issues"), ["issue", "issu"])
        self.assertEqual(coverage.suffix_forms("stopped"), ["stoppe", "stopp", "stop"])

    def test_no_form_shorter_than_two_letters_and_never_the_word(self):
        self.assertEqual(coverage.suffix_forms("as"), [])
        self.assertEqual(coverage.suffix_forms("is"), [])
        self.assertEqual(coverage.suffix_forms("bank"), [])
        self.assertNotIn("goes", coverage.suffix_forms("goes"))
        self.assertIn("go", coverage.suffix_forms("goes"))

    def test_a_doubled_vowel_is_not_undoubled(self):
        self.assertEqual(coverage.suffix_forms("seeing"), ["seee", "see"])
        self.assertNotIn("se", coverage.suffix_forms("seeing"))

    def test_exceptions_come_first_and_sorted(self):
        exceptions = {"went": ["go"], "leaves": ["leave", "leaf"]}
        self.assertEqual(coverage.reduce_word("went", exceptions), ["go"])
        self.assertEqual(coverage.reduce_word("leaves", exceptions)[:2], ["leaf", "leave"])
        self.assertEqual(coverage.reduce_word("leaves", exceptions).count("leave"), 1)
        self.assertEqual(coverage.reduce_word("went", {}), [])


class Parsers(unittest.TestCase):
    def test_wn_lmf(self):
        index, stats, exceptions = coverage.parse_wn_lmf(io.BytesIO(WN_LMF.encode()))
        self.assertEqual(stats, {"entries": 4, "headwords": 4})
        self.assertEqual(
            index["bank"], {"senses": 2, "definitions": 2, "synonyms": 1, "translations_ru": 0}
        )
        self.assertEqual(index["lonely"]["synonyms"], 0)
        self.assertEqual(exceptions, {"went": ["go"], "gone": ["go"]})

    def test_wn_lmf_wanted_limits_the_index_not_the_stats(self):
        index, stats, _ = coverage.parse_wn_lmf(io.BytesIO(WN_LMF.encode()), {"go"})
        self.assertEqual(list(index), ["go"])
        self.assertEqual(stats["entries"], 4)

    def test_wn_lmf_without_entries_is_an_error(self):
        with self.assertRaises(ValueError):
            coverage.parse_wn_lmf(io.BytesIO(b"<LexicalResource/>"))

    def test_freedict_tei_merges_homographs(self):
        index, stats = coverage.parse_freedict_tei(io.BytesIO(TEI.encode()))
        self.assertEqual(stats, {"entries": 3, "headwords": 2})
        self.assertEqual(
            index["bank"], {"senses": 3, "definitions": 3, "synonyms": 0, "translations_ru": 4}
        )
        # A translation into another language is not a Russian translation.
        self.assertEqual(index["go"]["translations_ru"], 0)
        self.assertEqual(index["go"]["definitions"], 0)

    def test_dictd(self):
        index_bytes, body = dictd_files(DICTD)
        index, stats = coverage.parse_dictd(io.BytesIO(index_bytes), body)
        self.assertEqual(stats, {"entries": 4, "headwords": 3})
        self.assertEqual(index["bank"]["senses"], 3)
        self.assertEqual(index["bank"]["definitions"], 4)
        self.assertEqual(index["bank"]["translations_ru"], 3)
        self.assertEqual(index["set off"]["senses"], 1)
        self.assertEqual(index["nodef"]["definitions"], 0)
        self.assertNotIn("00databaseinfo", index)

    def test_dictd_line_repeated_across_homographs_counts_once(self):
        index_bytes, body = dictd_files(
            [("bow", "bow <n>\nлук\nA weapon.\n"), ("bow", "bow <v>\nлук\n")]
        )
        index, _ = coverage.parse_dictd(io.BytesIO(index_bytes), body)
        self.assertEqual(index["bow"]["translations_ru"], 1)

    def test_dictd_skips_only_the_exact_info_headwords(self):
        index_bytes, body = dictd_files([("00databases", "00databases <n>\nбазы\n")])
        index, stats = coverage.parse_dictd(io.BytesIO(index_bytes), body)
        self.assertEqual(stats, {"entries": 1, "headwords": 1})
        self.assertIn("00databases", index)

    def test_size_is_compared_before_the_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "f"
            path.write_bytes(b"abc")
            candidate = {"size": 4, "sha256": coverage.sha256_of(path)}
            with self.assertRaises(coverage.HashMismatch) as caught:
                coverage.verify_file(candidate, path)
            self.assertIn("size differs", str(caught.exception))

    def test_dictd_number(self):
        self.assertEqual(coverage.dictd_number("A"), 0)
        self.assertEqual(coverage.dictd_number("BA"), 64)
        self.assertEqual(coverage.dictd_number("La/S"), int.from_bytes(base64.b64decode("La/S")))

    def test_dictd_article_outside_the_file_is_an_error(self):
        with self.assertRaises(ValueError):
            coverage.parse_dictd(io.BytesIO(b"bank\tA\tBA\n"), b"short")

    def test_kaikki(self):
        lines = [json.dumps(record, ensure_ascii=False) + "\n" for record in KAIKKI]
        index, stats = coverage.parse_kaikki(lines)
        # The German entry and the redirect are not English entries.
        self.assertEqual(stats, {"entries": 3, "headwords": 3})
        self.assertEqual(
            index["bank"], {"senses": 2, "definitions": 2, "synonyms": 2, "translations_ru": 2}
        )
        # A form-of or alt-of sense is a sense, not a definition.
        self.assertEqual(index["banks"]["senses"], 1)
        self.assertEqual(index["banks"]["definitions"], 0)
        self.assertEqual(index["color"]["definitions"], 0)

    def test_kaikki_bad_json_is_an_error(self):
        with self.assertRaises(ValueError):
            coverage.parse_kaikki(['{"word": "en"\n'])


class Run(unittest.TestCase):
    """The matrix rows, through `run` and `main` on files in a temporary
    directory."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.data = self.root / "data"
        self.data.mkdir()
        index_bytes, body = dictd_files(DICTD)
        files = {
            "oewn.xml.gz": gzip.compress(WN_LMF.encode()),
            "tei.tar.xz": tar_xz({"d/d.tei": TEI.encode()}),
            "dictd.tar.xz": tar_xz({"d/d.dict.dz": gzip.compress(body), "d/d.index": index_bytes}),
            "kaikki.jsonl.gz": kaikki_gz(),
        }
        formats = {
            "oewn.xml.gz": ("oewn", "wn-lmf-xml-gz", {}),
            "tei.tar.xz": ("tei", "freedict-tei-tar-xz", {"tei": "d/d.tei"}),
            "dictd.tar.xz": (
                "dictd",
                "freedict-dictd-tar-xz",
                {"dict": "d/d.dict.dz", "index": "d/d.index"},
            ),
            "kaikki.jsonl.gz": ("kaikki", "kaikki-jsonl-gz", {}),
        }
        candidates = []
        for file, data in files.items():
            (self.data / file).write_bytes(data)
            name, kind, members = formats[file]
            candidates.append(
                {
                    "name": name,
                    "file": file,
                    "url": "https://example.invalid/" + file,
                    "version": "t",
                    "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "format": kind,
                    "members": members,
                    "exception_source": kind == "wn-lmf-xml-gz",
                }
            )
        self.words_path = self.root / "words.txt"
        self.words_path.write_text(
            "# header\n# [general]\nbank\nlonely\nzyzzyva\n\n# [inflected]\nbanks\nwent\n",
            encoding="utf-8",
        )
        self.manifest = {
            "target_list": {
                "path": "unused",
                "sha256": coverage.sha256_of(self.words_path),
                "approved": "2026-10-11",
            },
            "candidates": candidates,
        }
        self.words = coverage.read_words(self.words_path)

    def rows(self, lines, candidate):
        return {
            line["word"]: line
            for line in lines
            if line["kind"] == "word" and line["candidate"] == candidate
        }

    def summary(self, lines, candidate):
        return next(
            line for line in lines if line["kind"] == "summary" and line["candidate"] == candidate
        )

    def run_lines(self):
        return coverage.run(self.manifest, self.words, self.data, "x")

    def test_word_found_as_written(self):
        row = self.rows(self.run_lines(), "kaikki")["bank"]
        self.assertTrue(row["found_as_written"])
        self.assertEqual(row["found_via"], [])
        self.assertEqual(
            row["as_written"],
            {"senses": 2, "definitions": 2, "synonyms": 2, "translations_ru": 2},
        )
        self.assertEqual(row["section"], "general")

    def test_found_only_by_dictionary_form(self):
        lines = self.run_lines()
        for candidate, word, form in [
            ("oewn", "banks", "bank"),
            ("oewn", "went", "go"),
            ("tei", "banks", "bank"),
            ("tei", "went", "go"),
        ]:
            row = self.rows(lines, candidate)[word]
            self.assertFalse(row["found_as_written"], (candidate, word))
            self.assertEqual(row["found_via"], [form])
            self.assertTrue(row["found_with_reduction"])
            self.assertEqual(row["as_written"]["senses"], 0)
            self.assertGreater(row["with_reduction"]["senses"], 0)
        summary = self.summary(lines, "oewn")
        self.assertEqual(summary["as_written"]["found"], 2)
        self.assertEqual(summary["with_reduction"]["found"], 4)
        self.assertEqual(summary["missed_as_written"], ["zyzzyva", "banks", "went"])
        self.assertEqual(summary["by_section"]["inflected"]["as_written"]["found"], 0)
        self.assertEqual(summary["by_section"]["inflected"]["with_reduction"]["found"], 2)

    def test_a_form_of_entry_is_found_but_gets_its_definition_by_reduction(self):
        row = self.rows(self.run_lines(), "kaikki")["banks"]
        self.assertTrue(row["found_as_written"])
        self.assertEqual(row["as_written"]["definitions"], 0)
        self.assertEqual(row["found_via"], ["bank"])
        self.assertEqual(row["with_reduction"]["definitions"], 2)

    def test_word_missing_everywhere_is_in_every_gap_list(self):
        lines = self.run_lines()
        for candidate in ("oewn", "tei", "dictd", "kaikki"):
            summary = self.summary(lines, candidate)
            self.assertIn("zyzzyva", summary["missed_as_written"])
            self.assertIn("zyzzyva", summary["missed_with_reduction"])

    def test_entry_without_a_field_is_found_with_the_field_absent(self):
        lines = self.run_lines()
        row = self.rows(lines, "oewn")["lonely"]
        self.assertTrue(row["found_as_written"])
        self.assertEqual(row["as_written"]["synonyms"], 0)
        summary = self.summary(lines, "oewn")
        self.assertEqual(summary["found_without_field_with_reduction"]["synonyms"], ["lonely", "went"])
        self.assertEqual(summary["with_reduction"]["found"], 4)
        self.assertEqual(summary["with_reduction"]["synonyms"], 2)
        # `go` has a German translation only: found, no Russian.
        row = self.rows(lines, "tei")["went"]
        self.assertTrue(row["found_with_reduction"])
        self.assertEqual(row["with_reduction"]["translations_ru"], 0)
        self.assertEqual(
            self.summary(lines, "tei")["found_without_field_with_reduction"]["translations_ru"],
            ["went"],
        )

    def test_oewn_is_never_an_en_ru_source(self):
        lines = self.run_lines()
        for row in self.rows(lines, "oewn").values():
            self.assertIsNone(row["as_written"]["translations_ru"])
            self.assertIsNone(row["with_reduction"]["translations_ru"])
        summary = self.summary(lines, "oewn")
        self.assertIsNone(summary["as_written"]["translations_ru"])
        self.assertIsNone(summary["with_reduction"]["translations_ru"])
        self.assertIsNone(summary["found_without_field_with_reduction"]["translations_ru"])
        self.assertNotIn("translations_ru", coverage.PROVIDES["wn-lmf-xml-gz"])

    def test_hash_mismatch_stops_the_run_and_names_file_and_both_hashes(self):
        path = self.data / "kaikki.jsonl.gz"
        expected = self.manifest["candidates"][3]["sha256"]
        path.write_bytes(path.read_bytes()[:-5])  # truncated download
        actual = coverage.sha256_of(path)
        with self.assertRaises(coverage.HashMismatch) as caught:
            self.run_lines()
        message = str(caught.exception)
        self.assertIn("kaikki.jsonl.gz", message)
        self.assertIn(expected, message)
        self.assertIn(actual, message)

    def test_hash_mismatch_gives_a_non_zero_exit_and_writes_nothing(self):
        path = self.data / "tei.tar.xz"
        expected = self.manifest["candidates"][1]["sha256"]
        path.write_bytes(b"changed")
        code, _, errors = self.main()
        self.assertEqual(code, 2)
        self.assertIn("tei.tar.xz", errors)
        self.assertIn(expected, errors)
        self.assertIn(coverage.sha256_of(path), errors)
        self.assertFalse((self.root / "results.jsonl").exists())

    def test_unavailable_candidate_is_unverified_and_the_run_goes_on(self):
        (self.data / "tei.tar.xz").unlink()
        self.manifest["candidates"][2] = {
            "name": "dictd",
            "status": "unverified",
            "reason": "HTTP 404 on 2026-10-11",
            "url": "https://example.invalid/gone",
        }
        lines = self.run_lines()
        self.assertEqual(self.summary(lines, "tei")["status"], "unverified")
        self.assertIn("file not found", self.summary(lines, "tei")["reason"])
        self.assertEqual(self.summary(lines, "dictd")["reason"], "HTTP 404 on 2026-10-11")
        for candidate in ("tei", "dictd"):
            self.assertEqual(self.rows(lines, candidate), {})
            self.assertNotIn("as_written", self.summary(lines, candidate))
        self.assertEqual(self.summary(lines, "kaikki")["status"], "verified")
        self.assertEqual(len(self.rows(lines, "kaikki")), 5)

    def test_unparsable_candidate_is_unverified_and_the_run_goes_on(self):
        self.replace("tei.tar.xz", tar_xz({"d/other.txt": b"x"}))
        self.replace("kaikki.jsonl.gz", gzip.compress(b"not json but en \"en\"\n"))
        self.manifest["candidates"][2]["format"] = "stardict"
        lines = self.run_lines()
        for candidate in ("tei", "kaikki"):
            self.assertEqual(self.summary(lines, candidate)["status"], "unverified")
            self.assertIn("cannot parse", self.summary(lines, candidate)["reason"])
        self.assertIn("unknown format", self.summary(lines, "dictd")["reason"])
        self.assertEqual(self.summary(lines, "oewn")["status"], "verified")

    def test_without_the_exception_source_irregular_forms_stay_missed(self):
        self.replace("oewn.xml.gz", gzip.compress(b"<broken"))
        lines = self.run_lines()
        self.assertEqual(self.summary(lines, "oewn")["status"], "unverified")
        self.assertIsNone(lines[0]["exception_source"])
        self.assertIn("went", self.summary(lines, "tei")["missed_with_reduction"])
        self.assertNotIn("banks", self.summary(lines, "tei")["missed_with_reduction"])

    def test_no_en_ru_candidate_covers_the_list_is_visible(self):
        summary = self.summary(self.run_lines(), "tei")
        self.assertEqual(summary["words"], 5)
        self.assertEqual(summary["with_reduction"]["translations_ru"], 2)
        self.assertEqual(summary["missed_with_reduction"], ["lonely", "zyzzyva"])

    def test_two_runs_give_identical_output(self):
        first = coverage.render(self.run_lines())
        self.assertEqual(first, coverage.render(self.run_lines()))
        self.assertEqual(self.main()[0], 0)
        written = (self.root / "results.jsonl").read_bytes()
        self.assertEqual(self.main()[0], 0)
        self.assertEqual(written, (self.root / "results.jsonl").read_bytes())
        kinds = [json.loads(line)["kind"] for line in written.decode().splitlines()]
        self.assertEqual(kinds.count("word"), 20)
        self.assertEqual(kinds.count("summary"), 4)
        self.assertEqual(kinds.count("candidate"), 4)

    def test_unapproved_or_changed_list_refuses_to_run(self):
        self.manifest["target_list"]["sha256"] = None
        code, _, errors = self.main()
        self.assertEqual(code, 3)
        self.assertIn("not approved", errors)
        self.manifest["target_list"]["sha256"] = "0" * 64
        code, _, errors = self.main()
        self.assertEqual(code, 3)
        self.assertIn("changed after approval", errors)
        self.assertFalse((self.root / "results.jsonl").exists())

    def test_list_rejects_duplicates_and_non_words(self):
        for text in ("bank\nbank\n", "well-known\n", "# only a comment\n"):
            self.words_path.write_text(text, encoding="utf-8")
            with self.assertRaises(ValueError):
                coverage.read_words(self.words_path)

    def replace(self, file, data):
        (self.data / file).write_bytes(data)
        for candidate in self.manifest["candidates"]:
            if candidate.get("file") == file:
                candidate["size"] = len(data)
                candidate["sha256"] = hashlib.sha256(data).hexdigest()

    def main(self):
        manifest_path = self.root / "candidates.json"
        manifest_path.write_text(json.dumps(self.manifest), encoding="utf-8")
        out, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(errors):
            code = coverage.main(
                [
                    "--candidates", str(manifest_path),
                    "--data", str(self.data),
                    "--words", str(self.words_path),
                    "--out", str(self.root / "results.jsonl"),
                ]
            )
        return code, out.getvalue(), errors.getvalue()


if __name__ == "__main__":
    unittest.main()
