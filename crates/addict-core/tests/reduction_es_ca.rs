//! Parity of the Spanish and Catalan reduction with `experiments/exp9/rules.py`:
//! `data/reduction-es-ca.tsv` was written once by `rules.reduce_word` (rules
//! only) over both EXP-9 target lists and the words of `test_rules.py`, for
//! both languages. Each line is `lang<TAB>word<TAB>forms joined by |`, and
//! the port must give every line exactly.

use addict_core::reduction_es_ca::{Language, forms};

#[test]
fn the_port_gives_what_rules_py_gives_for_every_golden_line() {
    let golden = include_str!("data/reduction-es-ca.tsv");
    let mut lines = 0;
    let mut differences = Vec::new();
    for line in golden.lines() {
        let mut fields = line.split('\t');
        let (Some(language), Some(word), Some(expected), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            panic!("malformed golden line {line:?}");
        };
        let language = match language {
            "es" => Language::Es,
            "ca" => Language::Ca,
            other => panic!("unknown language {other:?}"),
        };
        let expected: Vec<&str> = if expected.is_empty() {
            Vec::new()
        } else {
            expected.split('|').collect()
        };
        let actual = forms(word, language, |_| Vec::new());
        if actual != expected {
            differences.push(format!("{line}\n  port: {}", actual.join("|")));
        }
        lines += 1;
    }
    assert!(lines > 2000, "{lines} golden lines");
    assert!(
        differences.is_empty(),
        "{} of {lines} lines differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
