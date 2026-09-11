//! Two things ADR 0071 says do not exist, checked by the build rather than by
//! review
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md),
//! issue #1288).
//!
//! Both are absences, and an absence is exactly what a code review stops
//! noticing. Neither can be a unit test, because there is no value to assert
//! about: a float that should not be on the money path and a constant that
//! should not exist are both facts about the *source*, so the source is what
//! this reads.
//!
//! # Why these two
//!
//! **No floats.** This is ADR 0071's own first falsifier, quoted verbatim:
//! `grep -rE '\bf(32|64)\b' crates/connector-domain/src/` matching anything on
//! the conversion path falsifies decision 4's claim that the arithmetic is
//! integer-rational only. A rate expressed as a float is not a rounding
//! nuisance; it silently stops being the same number the connector declared,
//! and the whole safety argument -- the forward rounds down, the reject rounds
//! up, and the pair never understates a probed cost -- rests on exact integer
//! arithmetic. The grep is over the whole of `src/`, not just the conversion
//! path, because the crate has never held a float and the strongest check that
//! stays true is the cheapest one to keep.
//!
//! **No 1:1 rate as a constant.** ADR 0071 decision 2 makes silent conversion
//! structurally impossible by making *absence* the refusal: a pair with no
//! declared rate is rejected, loudly. That works only while a 1:1 rate is
//! something an operator has to write down deliberately. The moment
//! `Rate::IDENTITY` or `Rate::ONE` or a `Default` exists, the obvious fix for
//! "this pair has no rate" is to reach for it, and an unconverted
//! 18-vs-6-decimals pass-through -- a 10^12x error -- is one plausible-looking
//! line away. `Rate::new(1, 1)` is *not* forbidden and must not be: two
//! stablecoins at par is a real declaration, and refusing it would also break
//! decision 3's cross-rate composition whenever two tokens quote alike. What
//! is forbidden is a 1:1 rate nobody had to declare.

use std::fs;
use std::path::{Path, PathBuf};

/// ADR 0071's falsifier, as an expression rather than as a regex, because this
/// crate takes no regex dependency for one check: `f32` or `f64` bounded by
/// something that is not a word character on either side, which is what
/// `\bf(32|64)\b` means.
const FLOAT_TYPES: [&str; 2] = ["f32", "f64"];

/// The ways a 1:1 rate could be handed out without an operator declaring one,
/// matched as **declaration** substrings rather than as bare names: this
/// module and `rate.rs` both have to be able to say the words "IDENTITY" and
/// "Default" in prose in order to explain why neither exists.
const IDENTITY_DECLARATIONS: [&str; 7] = [
    "const IDENTITY",
    "const ONE",
    "impl Default for Rate",
    "(Default",
    ", Default",
    "fn identity(",
    "fn one(",
];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/`, sorted, so a failure names the same file
/// twice in a row.
fn source_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![src_dir()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("connector-domain has a src directory") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    assert!(
        !found.is_empty(),
        "no sources found under {}; a check that reads nothing passes for the wrong reason",
        src_dir().display()
    );
    found
}

/// `\b` around a match: the character before and after must not be one a
/// word can be made of.
fn is_a_whole_word(haystack: &str, at: usize, needle: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let before = haystack[..at].chars().next_back();
    let after = haystack[at + needle.len()..].chars().next();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

#[test]
fn no_float_appears_anywhere_on_the_value_path() {
    let mut offences = Vec::new();
    for path in source_files() {
        let source = fs::read_to_string(&path).expect("a readable source file");
        for (number, line) in source.lines().enumerate() {
            for float in FLOAT_TYPES {
                for (at, _) in line.match_indices(float) {
                    if is_a_whole_word(line, at, float) {
                        offences.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        offences.is_empty(),
        "ADR 0071 decision 4 claims the arithmetic is integer-rational only, and its own \
         falsifier is a float in this crate. Found:\n{}",
        offences.join("\n")
    );
}

#[test]
fn no_constant_hands_out_a_one_to_one_rate() {
    let rate_source = fs::read_to_string(src_dir().join("rate.rs")).expect("rate.rs is readable");
    // Everything below the test module is fixtures, and a test may name a
    // ratio of one legitimately -- what this checks is the type's own surface.
    let surface = rate_source
        .split("#[cfg(test)]")
        .next()
        .expect("rate.rs has a surface above its tests");

    for declaration in IDENTITY_DECLARATIONS {
        assert!(
            !surface.contains(declaration),
            "'{declaration}' would give a 1:1 rate a name nobody had to declare, and ADR 0071 \
             decision 2 makes absence the refusal. Write Rate::new(1, 1) at the call site if a \
             pair really is at par."
        );
    }
}
