"""EXP-9b: rules that reduce a Spanish (es) or Catalan (ca) word to the
forms its dictionary headword may have. No model, no part of speech: every
rule that matches gives a form, and a form is used only when it is a headword
of the dictionary being read (the caller checks that).

The input is a word as `addict_core::sentence::words()` and `lookup_word()`
give it: a run of letters in which an inner `'` (U+0027) or `·` (U+00B7)
stands between two letters. The middle dot is an ordinary character here: no
rule removes or adds it, so `col·leccions` reduces to `col·lecció`.

The reduction of `word` (`reduce_word`) is, in this order, without `word`
itself and without repeats:

1. the exception list: every lemma it gives for `word`, sorted;
2. for each base that `strip_bases` gives (Catalan elision, Spanish enclitic
   pronouns), in its order: the base, the lemmas the exception list gives for
   the base (sorted), and the suffix forms of the base;
3. the suffix forms of `word` (`suffix_forms`).

A form shorter than `MIN_FORM` letters is dropped. These functions are pure;
this module is what the product (epic entry 6) ports.
"""

from __future__ import annotations

MIN_FORM = 2

# Stress accents. The diaeresis (ü, ï), ñ and ç are letters of their own and
# are never changed.
STRIP = str.maketrans("áàéèíóòú", "aaeeioou")
ACCENTS = {
    "es": {"a": "á", "e": "é", "i": "í", "o": "ó", "u": "ú"},
    "ca": {"a": "à", "e": "èé", "i": "í", "o": "òó", "u": "ú"},
}

KEEP, STRIP_ACCENT, RESTORE = "keep", "strip", "restore"


def strip_accents(text: str) -> str:
    """`text` without stress accents: á à é è í ó ò ú become a a e e i o o u."""
    return text.translate(STRIP)


def accent_variants(form: str, lang: str) -> list[str]:
    """`form`, then `form` without accents, then that with one stress accent
    on one vowel, the last vowel first (Catalan e and o: grave, then acute)."""
    plain = strip_accents(form)
    variants = [form, plain]
    for at in range(len(plain) - 1, -1, -1):
        for accented in ACCENTS[lang].get(plain[at], ""):
            variants.append(plain[:at] + accented + plain[at + 1 :])
    return variants


# --------------------------------------------------------------------------
# Suffix rules: (suffix, replacement, accent mode). A rule matches when the
# word ends with the suffix and is longer than it; it gives the word with
# the suffix replaced. Mode `keep` gives that form; `strip` gives it without
# accents; `restore` gives every form of `accent_variants`.
# --------------------------------------------------------------------------


def _verb(endings: str, infinitives: str, mode: str = KEEP) -> list[tuple[str, str, str]]:
    return [(ending, infinitive, mode) for ending in endings.split() for infinitive in infinitives.split()]


def _future(endings: str, stems: str) -> list[tuple[str, str, str]]:
    """Future and conditional endings attach to the infinitive."""
    return [(stem + ending, stem, KEEP) for stem in stems.split() for ending in endings.split()]


ES_SUFFIX_RULES: tuple[tuple[str, str, str], ...] = tuple(
    [
        # plural and gender of nouns and adjectives
        ("ces", "z", KEEP),  # luces -> luz, felices -> feliz
        ("ones", "ón", KEEP),  # canciones -> canción
        ("es", "", KEEP),  # paredes -> pared, árboles -> árbol, países -> país
        ("es", "", STRIP_ACCENT),  # jóvenes -> joven, exámenes -> examen
        ("s", "", KEEP),  # bancos -> banco
        ("a", "o", KEEP),  # buena -> bueno
        ("as", "o", KEEP),  # buenas -> bueno
        ("a", "", KEEP),  # trabajadora -> trabajador, española -> español
        ("as", "", KEEP),  # españolas -> español
        # adverbs, superlatives, diminutives
        ("amente", "o", KEEP),  # rápidamente -> rápido
        ("mente", "", KEEP),  # fácilmente -> fácil, realmente -> real
        ("ísimo", "o", RESTORE),  # buenísimo -> bueno, rapidísimo -> rápido
        ("ísima", "o", RESTORE),
        ("ísimos", "o", RESTORE),
        ("ísimas", "o", RESTORE),
        ("ísimo", "", RESTORE),  # facilísimo -> fácil
        ("ísimo", "e", RESTORE),  # grandísimo -> grande
        ("quísimo", "co", KEEP),  # riquísimo -> rico
        ("guísimo", "go", KEEP),
        ("ito", "o", KEEP),  # perrito -> perro
        ("ita", "a", KEEP),  # casita -> casa
        ("itos", "o", KEEP),
        ("itas", "a", KEEP),
        ("quito", "co", KEEP),
        ("quita", "ca", KEEP),  # cerquita -> cerca
        ("guito", "go", KEEP),
        ("guita", "ga", KEEP),
        ("cito", "", KEEP),  # pastorcito -> pastor
        ("cita", "", KEEP),
    ]
    # -ar verbs: present, imperfect, preterite, subjunctive, imperative,
    # participle, gerund
    + _verb(
        "o as a amos áis an aba abas ábamos abais aban é aste ó asteis aron "
        "e es emos éis en ara aras áramos arais aran ase ases ásemos aseis asen "
        "ad ado ada ados adas ando",
        "ar",
    )
    # -er and -ir verbs share their endings; both infinitives are tried
    + _verb(
        "o es e emos éis en imos ís ía ías íamos íais ían í iste ió isteis ieron "
        "a as amos áis an iera ieras iéramos ierais ieran iese ieses iésemos ieseis iesen "
        "ed id ido ida idos idas iendo",
        "er ir",
    )
    + _future("é ás á emos éis án ía ías íamos íais ían", "ar er ir")
    # regular spelling changes
    + _verb("qué que ques quemos quen", "car")  # busqué -> buscar
    + _verb("gué gue gues guemos guen", "gar")  # llegué -> llegar
    + _verb("güé güe", "guar")
    + _verb("cé ce ces cemos cen", "zar")  # empecé -> empezar
    + _verb("jo ja jas jamos jan", "ger gir")  # cojo -> coger, dirijo -> dirigir
    + _verb("zo za zas zamos zan", "cer cir")  # venzo -> vencer
    + _verb("go ga gas gamos gan", "guir")  # distingo -> distinguir
    + _verb("yó yeron yendo", "er ir")  # leyó -> leer, huyendo -> huir
)

CA_SUFFIX_RULES: tuple[tuple[str, str, str], ...] = tuple(
    [
        # plural and gender of nouns and adjectives
        ("ons", "ó", KEEP),  # col·leccions -> col·lecció, il·lusions -> il·lusió
        ("ins", "í", KEEP),  # camins -> camí
        ("ans", "à", KEEP),  # mans -> mà
        ("ns", "", KEEP),  # bons -> bo
        ("ssos", "s", KEEP),  # gossos -> gos
        ("os", "", KEEP),  # braços -> braç, peixos -> peix, feliços -> feliç
        ("gues", "g", KEEP),  # llargues -> llarg
        ("gues", "ga", KEEP),
        ("ques", "c", KEEP),  # blanques -> blanc
        ("ques", "ca", KEEP),
        ("ges", "ja", KEEP),  # platges -> platja
        ("ces", "ça", KEEP),  # places -> plaça
        ("ves", "u", KEEP),  # noves -> nou
        ("va", "u", KEEP),  # nova -> nou
        ("es", "a", KEEP),  # cases -> casa
        ("es", "", KEEP),  # petites -> petit
        ("s", "", KEEP),  # bancs -> banc, nous -> nou
        ("a", "", KEEP),  # petita -> petit, blanca -> blanc
        # adverbs, superlatives, diminutives
        ("ament", "", KEEP),  # ràpidament -> ràpid
        ("ment", "", KEEP),  # fàcilment -> fàcil
        ("íssim", "", RESTORE),  # rapidíssim -> ràpid
        ("íssima", "", RESTORE),
        ("íssims", "", RESTORE),
        ("íssimes", "", RESTORE),
        ("et", "", KEEP),  # gatet -> gat
        ("et", "e", KEEP),  # llibret -> llibre
        ("eta", "a", KEEP),  # caseta -> casa
        ("ets", "", KEEP),
        ("etes", "a", KEEP),
    ]
    # -ar verbs
    + _verb(
        "o es a em eu en ava aves àvem àveu aven í ares à àrem àreu aren "
        "i is in és essis éssim éssiu essin at ada ats ades ant",
        "ar",
    )
    # inceptive -ir verbs
    + _verb("eix eixes eixen eixo eixi eixis eixin", "ir")  # serveix -> servir
    # -re, -er and -ir verbs: shared endings
    + _verb(
        "o es em eu en ia ies íem íeu ien i is in és essis éssim éssiu essin",
        "re er ir",
    )
    + _verb("ut uda uts udes ent", "re er")  # perdut -> perdre
    + _verb("it ida its ides int im iu", "ir")  # dormit -> dormir
    + _future("é às à em eu an ia ies íem íeu ien", "ar er ir")
    + _verb("ré ràs rà rem reu ran ria ries ríem ríeu rien", "re")  # perdré, perdria -> perdre
    # regular spelling changes of -ar verbs
    + _verb("cem ceu ci cis cin cés", "çar")  # comencem -> començar
    + _verb("gem geu gi gis gin gés", "jar")  # mengem -> menjar
    + _verb("quem queu qui quis quin qués", "car")  # toquem -> tocar
    + _verb("guem gueu gui guis guin gués", "gar")
)

# A Catalan verb in the third person singular present often has no ending:
# `perd` (perdre), `dorm` (dormir). When the word ends in a consonant and
# holds no `'`, these infinitive endings are added, after all suffix rules.
CA_BARE_STEM = ("re", "ir", "er")
VOWELS = set("aeiouàèéíòóúïü")

SUFFIX_RULES = {"es": ES_SUFFIX_RULES, "ca": CA_SUFFIX_RULES}


def _by_suffix(rules):
    table: dict[str, list[tuple[int, str, str]]] = {}
    for order, (suffix, replacement, mode) in enumerate(rules):
        table.setdefault(suffix, []).append((order, replacement, mode))
    return table, max(len(suffix) for suffix, _, _ in rules)


_TABLES = {lang: _by_suffix(rules) for lang, rules in SUFFIX_RULES.items()}


def suffix_forms(word: str, lang: str) -> list[str]:
    """The forms the suffix rules of `lang` give for `word`, in rule order,
    unique, each at least MIN_FORM long and not `word` itself."""
    table, longest = _TABLES[lang]
    matched: list[tuple[int, str, str, int]] = []
    for length in range(1, min(longest, len(word) - 1) + 1):
        for order, replacement, mode in table.get(word[-length:], ()):
            matched.append((order, replacement, mode, length))
    matched.sort()
    forms: list[str] = []

    def add(form: str) -> None:
        if len(form) >= MIN_FORM and form != word and form not in forms:
            forms.append(form)

    for _, replacement, mode, length in matched:
        form = word[: len(word) - length] + replacement
        if mode == KEEP:
            add(form)
        elif mode == STRIP_ACCENT:
            add(strip_accents(form))
        else:
            for variant in accent_variants(form, lang):
                add(variant)
    if lang == "ca" and "'" not in word and word[-1] not in VOWELS:
        for ending in CA_BARE_STEM:
            add(word + ending)
    return forms


# --------------------------------------------------------------------------
# Bases: Catalan elision and Spanish enclitic pronouns
# --------------------------------------------------------------------------

# An elided article, preposition or pronoun before the word, joined by `'`.
CA_ELISION_PREFIXES = ("l'", "d'", "s'", "m'", "t'", "n'")
# A reduced pronoun after a verb, joined by `'` (porta'l, dona'm, compra'ls).
CA_ELISION_SUFFIXES = ("'ls", "'ns", "'hi", "'ho", "'l", "'m", "'n", "'s", "'t")

ES_CLITICS = ("me", "te", "se", "nos", "os", "le", "les", "lo", "la", "los", "las")
ES_MAX_CLITICS = 3
ES_ACUTE = set("áéíóú")


def ca_bases(word: str) -> list[str]:
    """Without the reduced pronoun after the verb; without the elided prefix;
    without both (`d'aigua'l` -> `d'aigua`, `aigua'l`, `aigua`). Each base
    keeps any inner `'` or `·` of its own."""
    bases: list[str] = []
    fronts = [word] + [word[len(p) :] for p in CA_ELISION_PREFIXES if word.startswith(p)]
    for front in fronts:
        for candidate in [front] + [front[: -len(s)] for s in CA_ELISION_SUFFIXES if front.endswith(s)]:
            if candidate != word and len(candidate) >= MIN_FORM and candidate not in bases:
                bases.append(candidate)
    return bases


def es_bases(word: str) -> list[str]:
    """The verb under one, two or three enclitic pronouns (dá-me-lo). A base is
    kept when, without accents, it ends in `r` (infinitive) or `ndo` (gerund),
    or when it carries an acute accent (an imperative whose stress the
    pronouns moved: dá, levánta, cóme). Each base is given without accents,
    then as written if that differs (oírlo -> oír)."""
    bases: list[str] = []
    stems = [word]
    for _ in range(ES_MAX_CLITICS):
        stems = [
            stem[: -len(clitic)]
            for stem in stems
            for clitic in ES_CLITICS
            if stem.endswith(clitic) and len(stem) > len(clitic)
        ]
        for stem in stems:
            plain = strip_accents(stem)
            if not (plain.endswith("r") or plain.endswith("ndo") or ES_ACUTE & set(stem)):
                continue
            for base in (plain, stem):
                if base != word and len(base) >= MIN_FORM and base not in bases:
                    bases.append(base)
    return bases


def strip_bases(word: str, lang: str) -> list[str]:
    return ca_bases(word) if lang == "ca" else es_bases(word)


# --------------------------------------------------------------------------
# The reduction
# --------------------------------------------------------------------------


def reduce_word(word: str, lang: str, exceptions=None) -> list[str]:
    """Dictionary forms to try for `word` (module docstring). `exceptions`
    maps a form to its lemmas; None or empty means rules only."""
    exceptions = exceptions or {}
    forms: list[str] = []

    def add(items) -> None:
        for form in items:
            if form != word and len(form) >= MIN_FORM and form not in forms:
                forms.append(form)

    add(sorted(exceptions.get(word, ())))
    for base in strip_bases(word, lang):
        add([base])
        add(sorted(exceptions.get(base, ())))
        add(suffix_forms(base, lang))
    add(suffix_forms(word, lang))
    return forms


def rule_forms(word: str, lang: str) -> list[str]:
    """The reduction by rules alone: bases and suffix forms."""
    return reduce_word(word, lang, None)


def is_irregular(form: str, lemma: str, lang: str) -> bool:
    """Whether the pair belongs to the irregular-only exception list: the
    rules alone do not reach `lemma` from `form`."""
    return lemma != form and lemma not in rule_forms(form, lang)


def is_word(text: str) -> bool:
    """A word of the `sentence::words()` shape in NFC: letters, with an inner
    `'` or `·` only between two letters. `words()` also keeps combining marks
    (NFD text); this check, the lists and the rules assume NFC."""
    if not text or not text[0].isalpha() or not text[-1].isalpha():
        return False
    for at, character in enumerate(text):
        if character.isalpha():
            continue
        if character in "'·" and text[at - 1].isalpha() and text[at + 1].isalpha():
            continue
        return False
    return True
