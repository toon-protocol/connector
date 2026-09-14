//! A record number is taken once, and **0065 is the one exception there will
//! ever be**.
//!
//! [`docs/adr/README.md`](../../../docs/adr/README.md) opens with the rule the
//! whole folder rests on: *"The numbers are permanent and are never reused or
//! renumbered — they are cited over a thousand times across this repo and from
//! `toon-meta`, `relay` and `store`."* Nothing checked it, and on 2026-08-27 it
//! broke twice in one hour: `#1203` landed
//! `0065-a-price-is-a-schedule-over-payload-length.md` at 11:52 and `#1205`
//! landed `0065-mina-leaves-the-repository.md` at 12:59, both from branches cut
//! while 0064 was the folder's highest number. Neither review could see the
//! other's, `0066` went to the next record as though nothing had happened, and
//! the collision was found weeks later from another repository
//! ([#1249](https://github.com/toon-protocol/connector/issues/1249)).
//!
//! The index resolves it by **stating it rather than renumbering** — the
//! reasoning is there, not repeated here. What this harness adds is that the
//! resolution cannot be undone by accident in either direction:
//!
//! * a *third* record at 0065, or a first at any other repeated number, fails
//!   here rather than in somebody else's citation months later;
//! * and the index is required to keep naming both halves, because a stated
//!   collision that stops being stated is worse than one nobody noticed — every
//!   reader after that point is told there is one 0065.
//!
//! What this cannot catch is the same class the falsifier harness names about
//! itself: whether a bare `ADR 0065` in prose means the right one of the two.
//! That is a human's job, and the index says which form to write instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The two records that share 0065, by filename. Deliberately a literal pair
/// rather than "any two files at the same number": the exception is these two
/// documents, not a quota of one collision that a future pair may spend.
const SHARED_0065: [&str; 2] = [
    "0065-a-price-is-a-schedule-over-payload-length.md",
    "0065-mina-leaves-the-repository.md",
];

/// The short forms the index tells a citing document to use. If these stop
/// appearing there, the index has stopped disambiguating and every downstream
/// citation is back to guessing.
const SHORT_FORMS: [&str; 2] = ["0065-price", "0065-mina"];

fn adr_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/adr")
        .canonicalize()
        .expect("docs/adr must be reachable from crates/connector-bin")
}

/// Every record in the folder, as `number -> filenames`. `README.md` is the
/// index, not a record, and carries no number.
fn records_by_number() -> BTreeMap<String, Vec<String>> {
    let mut by_number: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let entries = std::fs::read_dir(adr_dir()).expect("docs/adr must be readable");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") || name == "README.md" {
            continue;
        }
        let (number, rest) = name.split_at(4.min(name.len()));
        assert!(
            number.len() == 4
                && number.chars().all(|c| c.is_ascii_digit())
                && rest.starts_with('-'),
            "docs/adr/{name} is not named `NNNN-slug.md`. Every record in this folder is \
             numbered, and the number is how it is cited from three other repositories; a \
             record without one cannot be cited at all."
        );
        by_number.entry(number.to_string()).or_default().push(name);
    }
    assert!(
        by_number.len() > 60,
        "found only {} numbered record(s) under docs/adr — the folder has moved and this \
         harness is reading the wrong directory, which would pass while checking nothing",
        by_number.len()
    );
    by_number
}

#[test]
fn no_number_is_taken_twice_except_0065() {
    let by_number = records_by_number();

    let mut collisions: Vec<(String, Vec<String>)> = by_number
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .collect();
    for (_, files) in collisions.iter_mut() {
        files.sort();
    }

    let expected_0065: Vec<String> = SHARED_0065.iter().map(|s| s.to_string()).collect();
    let expected = vec![("0065".to_string(), expected_0065)];

    assert_eq!(
        collisions, expected,
        "the ADR numbering has changed.\n\n\
         Exactly one number in docs/adr is taken twice — 0065, by\n  {}\nand\n  {}\n\
         — and docs/adr/README.md states that collision and why neither record is renumbered.\n\n\
         If a NEW pair collides: renumber the record that has not landed on `main` yet. That is \
         the only moment renumbering is free, and it is why this gate fires on a branch rather \
         than in a citation from `toon-meta` months later.\n\n\
         If 0065 itself changed: it must not have. Both numbers are cited by number and by \
         filename from outside this repository, and the index's own note explains why a stub \
         and a redirect do not repair a sentence somebody already wrote.",
        SHARED_0065[0], SHARED_0065[1]
    );
}

#[test]
fn the_index_still_names_both_halves_of_0065() {
    let index = std::fs::read_to_string(adr_dir().join("README.md"))
        .expect("docs/adr/README.md must be readable");

    for file in SHARED_0065 {
        assert!(
            index.contains(file),
            "docs/adr/README.md no longer links `{file}`. Both records numbered 0065 have a row \
             in this index; dropping one is how the folder would come to look as though the \
             number were taken once."
        );
    }
    for form in SHORT_FORMS {
        assert!(
            index.contains(form),
            "docs/adr/README.md no longer offers the short form `{form}`. The resolution of the \
             0065 collision is that citations disambiguate; the index is where the form to use \
             is defined, so it has to keep defining it."
        );
    }
}
