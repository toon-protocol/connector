//! Every `eth_getLogs` this crate sends is built by `log_query::scoped_event`,
//! which names the emitting contract's address. See that module for why: a
//! bare `event::<D>()`, or any abigen-generated `*_filter()` helper built on
//! it, drops the address, and public RPCs refuse the query. It shipped twice
//! (#970, #1367), each time green against `anvil`.
//!
//! This reads the crate's own sources and fails on any other way of building
//! an event query, so a third copy of the defect is a red build rather than a
//! devnet outage.

use std::path::{Path, PathBuf};

/// The one file allowed to build an event query.
const THE_ONE_BUILDER: &str = "src/log_query.rs";

/// Calls that build or send a log query: `event` (however it is spelled)
/// drops the address, the generated helpers are `event` underneath, the
/// scoped siblings are still a second way to do what `scoped_event` does, and
/// a raw `get_logs` skips the binding altogether.
const QUERY_BUILDERS: &[&str] = &[
    ".event::<",
    "::event::<",
    ".event(",
    "_filter()",
    "_filter ()",
    "get_logs(",
    ".event_with_filter(",
    ".event_for_name(",
];

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_source_builds_an_event_query_except_scoped_event() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    rust_sources(&root.join("src"), &mut sources);
    assert!(
        sources.iter().any(|path| path.ends_with(THE_ONE_BUILDER)),
        "{THE_ONE_BUILDER} moved; update this guard with it"
    );

    let mut offences = Vec::new();
    for path in &sources {
        if path.ends_with(THE_ONE_BUILDER) {
            continue;
        }
        let text = std::fs::read_to_string(path).expect("read source");
        for (number, line) in text.lines().enumerate() {
            if QUERY_BUILDERS.iter().any(|builder| line.contains(builder)) {
                offences.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(root).unwrap_or(path).display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "build event queries with log_query::scoped_event, which names the contract's \
         address:\n{}",
        offences.join("\n")
    );
}
