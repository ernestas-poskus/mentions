# mentions

A small, dependency-light crate that parses and validates `@`-mention
references in plain text: files, directories, line numbers, line ranges, globs
and home-absolute paths. For every token it also reports the byte range it
occupied, so a caller can strip the raw token and splice resolved content in
place.

[![crates.io](https://img.shields.io/crates/v/mentions.svg)](https://crates.io/crates/mentions)
[![docs.rs](https://docs.rs/mentions/badge.svg)](https://docs.rs/mentions)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`@`-mentions are the convention many AI coding tools use to attach context to a
prompt. This crate is the syntax layer only: it extracts and validates the
tokens; resolving them against a filesystem and deciding what may be read stay
with the caller.

## Install

```toml
[dependencies]
mentions = "0.1"
```

## Quick start

```rust
use mentions::{Mention, parse, strip_mentions};

let prompt = "Review @src/lib.rs:10-20 and the @docs/ folder.";

for (range, mention) in parse(prompt) {
    println!("{:?} at {:?} -> {}", mention, range, mention.path());
}

let mentions = mentions::parse_mentions(prompt);
assert!(matches!(mentions[0], Mention::FileRange(_)));
assert_eq!(strip_mentions(prompt), "Review  and the  folder.");
```

Run it directly:

```console
cargo run --example basic
```

## Syntax

| Token | Meaning |
| --- | --- |
| `@path/to/file` | a file |
| `@path/to/dir/` | a directory (trailing slash) |
| `@path/to/file:LINE` | a single line (`LINE >= 1`) |
| `@path/to/file:START-END` | an inclusive line range |
| `@path/**/*.rs` | a glob (`*`, `?`, `[`) |
| `@~/path` | home-absolute path, preserved as written |

Mentions are recognized only at the start of the text or after whitespace, so
`bob@example.com` is prose, not a mention. Trailing sentence punctuation
(`, . ; : ! ? ) ] } " '`) is not part of the token. A bare `@` and a root-only
`@/` produce no mention.

## Validation

`Mention::validate()` uses [`validator`](https://docs.rs/validator) derives and
rejects:

- empty paths;
- NUL bytes;
- `..` parent-directory segments;
- `line`, `start` or `end` below `1`.

The cross-field rule `start <= end` is exposed separately as
`FileRangeMention::validate_ordering`, because a derive cannot express it.

This is **syntax validation only**. Whether a path may be read, and from which
root, is the caller's decision — resolving mentions is deliberately not part of
this crate.

## API

| Item | Purpose |
| --- | --- |
| `parse(&str) -> Vec<(Range<usize>, Mention)>` | mentions plus the byte range each occupied |
| `parse_mentions(&str) -> Vec<Mention>` | mentions only |
| `strip_mentions(&str) -> String` | text with every mention token removed |
| `Mention` | `File`, `Directory`, `FileLine`, `FileRange`, `Glob` |
| `Mention::path()` | the referenced path, or the glob pattern |
| `Mention::validate()` | validator-derived syntax validation |

## Design notes

- **Parser:** [`winnow`](https://docs.rs/winnow) 1.x, a maintained
  parser-combinator library; the grammar is a small token scan, not a general
  language.
- **Validation:** [`validator`](https://docs.rs/validator) derives keep the
  per-field rules (length, ranges, custom predicates) declarative.
- **Shape:** the token set is intentionally small — file, directory, line,
  range and glob. Symbol/thread/fetch/image mentions are out of scope.

## Minimum supported Rust version

Rust 1.88 (set by `validator` 0.21; `winnow` needs 1.65).

## License

MIT. See [LICENSE](LICENSE).
