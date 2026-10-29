//! Parse mentions from a prompt, print their byte ranges, and strip them.
//!
//! Run with `cargo run --example basic`.

use mentions::{parse, parse_mentions, strip_mentions, Mention};

fn main() {
    let prompt = "Review @src/lib.rs:10-20 and the @docs/ folder.";

    for (range, mention) in parse(prompt) {
        println!(
            "{mention:?} at bytes {range:?} -> path {:?}",
            mention.path()
        );
    }

    let parsed = parse_mentions(prompt);
    assert_eq!(parsed.len(), 2);
    assert!(matches!(parsed[0], Mention::FileRange(_)));
    assert!(matches!(parsed[1], Mention::Directory(_)));

    // Validation is syntax-only: this crate does not resolve paths.
    let ok = Mention::File(mentions::FileMention {
        path: "src/lib.rs".into(),
    });
    assert!(ok.validate().is_ok());

    let traversal = Mention::File(mentions::FileMention {
        path: "../secret".into(),
    });
    assert!(traversal.validate().is_err());

    println!("stripped: {:?}", strip_mentions(prompt));
}
