"""Tests of the ES/CA dictionary-form rules (rules.py) and of the coverage
helpers that use them: every row of the I/O matrix of the EXP-9b plan and
each rule family. Inline fixtures only; no dictionary file is read."""

import hashlib
import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import rules  # noqa: E402


def _load_coverage():
    # Loaded under its own name: EXP-7's tests import their `coverage` module
    # in the same run.
    spec = importlib.util.spec_from_file_location("exp9_coverage", HERE / "coverage.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


coverage = _load_coverage()


def reduce(word, lang, exceptions=None):
    return rules.reduce_word(word, lang, exceptions)


class MatrixTest(unittest.TestCase):
    """One test per row of the plan's I/O & edge-case matrix."""

    def test_exact_hit(self):
        index = {"banco": {"ru": 1}}
        row = coverage.look_up("banco", index, {"rules": reduce("banco", "es")}, {"ru"})
        self.assertTrue(row["as_written"]["found"])
        self.assertEqual(row["as_written"]["kinds"]["ru"], 1)
        self.assertNotIn("definitions", row["as_written"]["kinds"])
        summary = coverage.summarize("t", "es", [dict(row, word="banco", section="general")], ["as_written", "rules"], {"ru"})
        self.assertEqual(summary["variants"]["as_written"]["found"], 1)
        self.assertEqual(summary["variants"]["as_written"]["ru"], 1)

    def test_rule_hit(self):
        self.assertIn("cerrar", reduce("cerró", "es"))
        index = {"cerrar": {"ru": 2}}
        row = coverage.look_up("cerró", index, {"rules": reduce("cerró", "es")}, {"ru"})
        self.assertFalse(row["as_written"]["found"])
        self.assertEqual(row["variants"]["rules"]["via"], ["cerrar"])
        rows = [dict(row, word="cerró", section="forms")]
        summary = coverage.summarize("t", "es", rows, ["as_written", "rules"], {"ru"})
        self.assertEqual(summary["variants"]["as_written"]["found"], 0)
        self.assertEqual(summary["variants"]["rules"]["found"], 1)
        self.assertEqual(summary["variants"]["rules"]["ru"], 1)
        self.assertEqual(summary["variants"]["as_written"]["missed"], ["cerró"])

    def test_elision(self):
        self.assertEqual(rules.ca_bases("l'home"), ["home"])
        self.assertEqual(reduce("l'home", "ca")[0], "home")
        index = {"home": {"en": 1}}
        row = coverage.look_up("l'home", index, {"rules": reduce("l'home", "ca")}, {"en"})
        self.assertFalse(row["as_written"]["found"])
        self.assertEqual(row["variants"]["rules"]["via"], ["home"])

    def test_middle_dot(self):
        forms = rules.suffix_forms("col·leccions", "ca")
        self.assertIn("col·leccion", forms)  # the -s rule
        self.assertTrue(all("·" in form for form in forms))
        exceptions = {"col·leccions": ["col·lecció"]}
        self.assertEqual(reduce("col·leccions", "ca", exceptions)[0], "col·lecció")

    def test_enclitic(self):
        forms = reduce("dámelo", "es")
        self.assertIn("dar", forms)
        self.assertIn("da", forms)
        self.assertLess(forms.index("da"), forms.index("dar"))
        index = {"dar": {"ru": 1}, "dama": {"ru": 1}}
        row = coverage.look_up("dámelo", index, {"rules": forms}, {"ru"})
        self.assertEqual(row["variants"]["rules"]["via"], ["dar"])  # only headwords are kept

    def test_hash_differs(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "f.txt"
            path.write_bytes(b"changed")
            actual = hashlib.sha256(b"changed").hexdigest()
            entry = {"file": "f.txt", "size": 7, "sha256": "0" * 64}
            with self.assertRaises(coverage.Stop) as caught:
                coverage.check_file(entry, Path(scratch))
            self.assertEqual(caught.exception.code, 2)
            self.assertIn("0" * 64, str(caught.exception))
            self.assertIn(actual, str(caught.exception))
            entry = {"file": "f.txt", "size": 9, "sha256": actual}
            with self.assertRaises(coverage.Stop) as caught:
                coverage.check_file(entry, Path(scratch))
            self.assertEqual(caught.exception.code, 2)
            with self.assertRaises(coverage.Stop) as caught:
                coverage.check_list(path, "1" * 64)
            self.assertEqual(caught.exception.code, 3)
            self.assertIn("1" * 64, str(caught.exception))
            self.assertIn(actual, str(caught.exception))
            self.assertEqual(coverage.check_list(path, actual), actual)

    def test_morfologik_classpath_missing(self):
        rows = {"spanish-pos-dict-2.5": {"name": "spanish-pos-dict-2.5", "format": "morfologik-jar"}}
        skipped = {}
        with redirect_stderr(io.StringIO()):
            pairs = coverage.read_form_sources("es", ["spanish-pos-dict-2.5"], rows, {}, None, skipped, {}, {})
        self.assertEqual(pairs, {})  # left out, not an empty list
        self.assertIn("MORFOLOGIK_CP", skipped["spanish-pos-dict-2.5"])
        out = io.StringIO()
        coverage.print_summary([{"kind": "run", "skipped": skipped}], out)
        self.assertIn("SKIPPED spanish-pos-dict-2.5", out.getvalue())


class SpanishRulesTest(unittest.TestCase):
    def assertReaches(self, word, lemma, lang="es"):
        self.assertIn(lemma, reduce(word, lang), f"{word} -> {lemma}")

    def test_plural_and_gender(self):
        for word, lemma in [
            ("bancos", "banco"),
            ("luces", "luz"),
            ("felices", "feliz"),
            ("canciones", "canción"),
            ("jóvenes", "joven"),
            ("exámenes", "examen"),
            ("árboles", "árbol"),
            ("países", "país"),
            ("buena", "bueno"),
            ("buenas", "bueno"),
            ("trabajadora", "trabajador"),
            ("españolas", "español"),
        ]:
            self.assertReaches(word, lemma)

    def test_verbs(self):
        for word, lemma in [
            ("habló", "hablar"),
            ("hablábamos", "hablar"),
            ("hablaré", "hablar"),
            ("hablaría", "hablar"),
            ("hablando", "hablar"),
            ("cerrada", "cerrar"),
            ("comía", "comer"),
            ("comiera", "comer"),
            ("viviré", "vivir"),
            ("vivimos", "vivir"),
            ("viviendo", "vivir"),
            ("subieron", "subir"),
            ("leyó", "leer"),
        ]:
            self.assertReaches(word, lemma)
        self.assertIn("comir", reduce("comía", "es"))  # -er and -ir are both tried

    def test_spelling_changes(self):
        for word, lemma in [
            ("busqué", "buscar"),
            ("llegué", "llegar"),
            ("empecé", "empezar"),
            ("almorcé", "almorzar"),
            ("cojo", "coger"),
            ("dirijo", "dirigir"),
            ("venzo", "vencer"),
            ("distingo", "distinguir"),
        ]:
            self.assertReaches(word, lemma)

    def test_derived(self):
        for word, lemma in [
            ("rápidamente", "rápido"),
            ("fácilmente", "fácil"),
            ("rapidísimo", "rápido"),
            ("buenísimo", "bueno"),
            ("facilísimo", "fácil"),
            ("casita", "casa"),
            ("perrito", "perro"),
            ("cerquita", "cerca"),
        ]:
            self.assertReaches(word, lemma)

    def test_enclitics(self):
        for word, lemma in [
            ("comprarlo", "comprar"),
            ("sentarse", "sentar"),
            ("levántate", "levantar"),
            ("mirándola", "mirar"),
            ("explicárselo", "explicar"),
            ("cómetelo", "comer"),
        ]:
            self.assertReaches(word, lemma)
        self.assertEqual(rules.es_bases("oírlo")[:2], ["oir", "oír"])
        self.assertEqual(rules.es_bases("pala"), [])  # no r, ndo or acute accent
        self.assertEqual(rules.es_bases("bancos"), [])

    def test_order_and_uniqueness(self):
        exceptions = {"fue": ["ser", "ir"]}
        forms = reduce("fue", "es", exceptions)
        self.assertEqual(forms[:2], ["ir", "ser"])  # exceptions first, sorted
        self.assertEqual(len(forms), len(set(forms)))
        self.assertNotIn("fue", forms)
        self.assertTrue(all(len(form) >= rules.MIN_FORM for form in reduce("as", "es")))

    def test_irregular_only(self):
        self.assertTrue(rules.is_irregular("fue", "ser", "es"))
        self.assertFalse(rules.is_irregular("cerró", "cerrar", "es"))
        self.assertFalse(rules.is_irregular("dámelo", "dar", "es"))


class CatalanRulesTest(unittest.TestCase):
    def assertReaches(self, word, lemma):
        self.assertIn(lemma, reduce(word, "ca"), f"{word} -> {lemma}")

    def test_plural_and_gender(self):
        for word, lemma in [
            ("cases", "casa"),
            ("bancs", "banc"),
            ("camins", "camí"),
            ("mans", "mà"),
            ("bons", "bo"),
            ("gossos", "gos"),
            ("braços", "braç"),
            ("blanques", "blanc"),
            ("llargues", "llarg"),
            ("noves", "nou"),
            ("nova", "nou"),
            ("petites", "petit"),
            ("petita", "petit"),
            ("il·lusions", "il·lusió"),
        ]:
            self.assertReaches(word, lemma)

    def test_verbs(self):
        for word, lemma in [
            ("tancà", "tancar"),
            ("tancaren", "tancar"),
            ("parlàvem", "parlar"),
            ("parlés", "parlar"),
            ("parlaria", "parlar"),
            ("perdut", "perdre"),
            ("perdent", "perdre"),
            ("perdria", "perdre"),
            ("perd", "perdre"),
            ("dorm", "dormir"),
            ("dormint", "dormir"),
            ("serveix", "servir"),
            ("serveixen", "servir"),
            ("rebem", "rebre"),
        ]:
            self.assertReaches(word, lemma)

    def test_spelling_changes(self):
        for word, lemma in [
            ("comencem", "començar"),
            ("comenci", "començar"),
            ("mengem", "menjar"),
            ("toquem", "tocar"),
            ("pugem", "pujar"),
            ("busqui", "buscar"),
        ]:
            self.assertReaches(word, lemma)

    def test_derived(self):
        for word, lemma in [
            ("ràpidament", "ràpid"),
            ("fàcilment", "fàcil"),
            ("rapidíssim", "ràpid"),
            ("llibret", "llibre"),
            ("caseta", "casa"),
            ("gatet", "gat"),
        ]:
            self.assertReaches(word, lemma)

    def test_elision(self):
        for word, lemma in [
            ("d'art", "art"),
            ("d'aigua", "aigua"),
            ("s'ha", "ha"),
            ("n'hi", "hi"),
            ("l'il·lusió", "il·lusió"),
            ("d'intel·ligència", "intel·ligència"),
            ("porta'l", "portar"),
            ("dona'm", "donar"),
            ("compra'ls", "comprar"),
            ("s'acosta", "acostar"),
        ]:
            self.assertReaches(word, lemma)
        self.assertEqual(rules.ca_bases("casa"), [])
        self.assertEqual(rules.ca_bases("d'aigua'l"), ["d'aigua", "aigua'l", "aigua"])
        # The bare-stem rule never applies across an apostrophe.
        self.assertNotIn("porta'lre", reduce("porta'l", "ca"))

    def test_exceptions_on_a_base(self):
        exceptions = {"havia": ["haver"]}
        self.assertIn("haver", reduce("n'havia", "ca", exceptions))


class HelpersTest(unittest.TestCase):
    def test_accents(self):
        self.assertEqual(rules.strip_accents("àéèíóòú ü ï ñ ç"), "aeeioou ü ï ñ ç")
        self.assertIn("rápido", rules.accent_variants("rapido", "es"))
        self.assertIn("ràpid", rules.accent_variants("rapid", "ca"))
        self.assertIn("cafè", rules.accent_variants("cafe", "ca"))
        self.assertIn("café", rules.accent_variants("cafe", "ca"))

    def test_is_word(self):
        for text in ("banco", "l'home", "col·lecció", "porta'l"):
            self.assertTrue(rules.is_word(text), text)
        for text in ("", "'home", "home'", "l''home", "col··lecció", "well-known", "a b", "x1"):
            self.assertFalse(rules.is_word(text), text)

    def test_exception_tables(self):
        pairs = {("fue", "ser"), ("fue", "ir"), ("cerró", "cerrar"), ("casas", "casa")}
        by_form, stats = coverage.table_stats(pairs, "es")
        self.assertEqual(stats["full_pairs"], 4)
        self.assertEqual(stats["full_forms"], 3)
        self.assertEqual(stats["irregular_pairs"], 2)  # fue -> ser, fue -> ir
        self.assertEqual(stats["irregular_forms"], 1)
        full, irregular = coverage.variants_for(by_form, {"fue", "cerró", "nada"}, "es")
        self.assertEqual(full, {"fue": ["ir", "ser"], "cerró": ["cerrar"]})
        self.assertEqual(irregular, {"fue": ["ir", "ser"]})
        # Rules plus either list reach the same forms.
        self.assertEqual(set(reduce("cerró", "es", full)), set(reduce("cerró", "es", irregular)))

    def test_union_and_false_hits(self):
        def row(found, via=None):
            result = {"as_written": {"found": found, "kinds": {"ru": int(found)}}, "variants": {}}
            if via:
                result["variants"]["rules"] = {"via": via, "kinds": {"ru": 1}}
            return result

        rows_all = {
            "es": {"ca": {"a": {"casa": row(True)}, "b": {"cases": row(False, ["casa"])}}},
            "ca": {"es": {}},
        }
        lists = {"es": [], "ca": [("casa", "general"), ("cases", "forms"), ("gat", "general")], "en": []}
        variants = {"es": ["as_written", "rules"], "ca": ["as_written", "rules"]}
        out = coverage.false_hits(rows_all, {}, lists, variants, {})
        es_ca = next(line for line in out if line["lookup"] == "es" and line["list"] == "ca")
        self.assertEqual(es_ca["exact"], ["casa"])
        self.assertEqual(es_ca["rule_only"], {"rules": ["cases"]})
        chosen = coverage.false_hits(rows_all, {}, lists, variants, {}, chosen={"es": ["b"]})
        es_ca = next(line for line in chosen if line["lookup"] == "es" and line["list"] == "ca")
        self.assertEqual(es_ca["exact"], [])
        self.assertEqual(es_ca["rule_only"], {"rules": ["cases"]})


if __name__ == "__main__":
    unittest.main()
