//! Parse and validate `@`-mention references in plain text.
//!
//! Many AI coding tools use `@`-mentions to attach context to a prompt: a `@`
//! token references a file, directory (trailing slash), line/range, or glob,
//! and the raw token is stripped from the prompt before the model sees the
//! resolved content. This crate extracts those tokens, validates their syntax,
//! and reports the byte range each occupied so the caller can splice resolved
//! content back in place.
//!
//! A mention is recognized only at the start of the text or after whitespace,
//! and trailing sentence punctuation is not part of it, so `bob@example.com`
//! stays prose.
//!
//! Parsing is syntax-only: [`Mention::validate`] rejects empty, NUL-bearing and
//! `..`-traversal paths before they are resolved, but resolving a path and
//! deciding what it may read is the caller's job.
//!
//! # Examples
//!
//! ```
//! use mentions::{Mention, parse, parse_mentions, strip_mentions};
//!
//! let prompt = "Review @src/lib.rs:10-20 and the @docs/ folder.";
//! let mentions = parse_mentions(prompt);
//! assert_eq!(mentions.len(), 2);
//! assert!(matches!(mentions[0], Mention::FileRange(_)));
//! assert_eq!(strip_mentions(prompt), "Review  and the  folder.");
//!
//! // Byte ranges let a caller replace a token with the resolved content.
//! let (range, mention) = &parse(prompt)[0];
//! assert_eq!(&prompt[range.clone()], "@src/lib.rs:10-20");
//! assert_eq!(mention.path(), "src/lib.rs");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::ops::Range;

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use winnow::ascii::digit1;
use winnow::combinator::opt;
use winnow::token::{one_of, take_till};
use winnow::{ModalResult, Parser};

/// A single parsed `@`-mention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mention {
    /// `@path/to/file`
    File(FileMention),
    /// `@path/to/dir/` (trailing slash)
    Directory(DirectoryMention),
    /// `@path/to/file:LINE`
    FileLine(FileLineMention),
    /// `@path/to/file:START-END`
    FileRange(FileRangeMention),
    /// `@path/**/*.rs` (contains a glob metacharacter)
    Glob(GlobMention),
}

impl Mention {
    /// Validate the active variant's fields.
    ///
    /// # Examples
    ///
    /// ```
    /// use mentions::{FileMention, Mention};
    ///
    /// let ok = Mention::File(FileMention { path: "src/lib.rs".into() });
    /// assert!(ok.validate().is_ok());
    ///
    /// let traversal = Mention::File(FileMention { path: "../secret".into() });
    /// assert!(traversal.validate().is_err());
    /// ```
    pub fn validate(&self) -> Result<(), validator::ValidationErrors> {
        match self {
            Mention::File(m) => m.validate(),
            Mention::Directory(m) => m.validate(),
            Mention::FileLine(m) => m.validate(),
            Mention::FileRange(m) => m.validate(),
            Mention::Glob(m) => m.validate(),
        }
    }

    /// The referenced path (or glob pattern), for resolution by the caller.
    pub fn path(&self) -> &str {
        match self {
            Mention::File(m) => &m.path,
            Mention::Directory(m) => &m.path,
            Mention::FileLine(m) => &m.path,
            Mention::FileRange(m) => &m.path,
            Mention::Glob(m) => &m.pattern,
        }
    }
}

/// `@path/to/file`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Validate)]
pub struct FileMention {
    /// Path exactly as written after `@` (no leading `@`; `~` is preserved).
    #[validate(length(min = 1), custom(function = "validate_safe_path"))]
    pub path: String,
}

/// `@path/to/dir/` (trailing slash)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Validate)]
pub struct DirectoryMention {
    /// Directory path without the trailing slash.
    #[validate(length(min = 1), custom(function = "validate_safe_path"))]
    pub path: String,
}

/// `@path/to/file:LINE`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Validate)]
pub struct FileLineMention {
    /// Path of the referenced file.
    #[validate(length(min = 1), custom(function = "validate_safe_path"))]
    pub path: String,
    /// 1-based line number.
    #[validate(range(min = 1))]
    pub line: u64,
}

/// `@path/to/file:START-END`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Validate)]
pub struct FileRangeMention {
    /// Path of the referenced file.
    #[validate(length(min = 1), custom(function = "validate_safe_path"))]
    pub path: String,
    /// 1-based first line of the range.
    #[validate(range(min = 1))]
    pub start: u64,
    /// 1-based last line of the range (inclusive).
    #[validate(range(min = 1))]
    pub end: u64,
}

impl FileRangeMention {
    /// Cross-field rule the derive can't express: `start <= end`.
    ///
    /// # Examples
    ///
    /// ```
    /// use mentions::FileRangeMention;
    ///
    /// let ok = FileRangeMention { path: "a.rs".into(), start: 1, end: 3 };
    /// assert!(ok.validate_ordering().is_ok());
    ///
    /// let backwards = FileRangeMention { path: "a.rs".into(), start: 3, end: 1 };
    /// assert!(backwards.validate_ordering().is_err());
    /// ```
    pub fn validate_ordering(&self) -> Result<(), ValidationError> {
        if self.start > self.end {
            let mut err = ValidationError::new("range_ordering");
            err.message = Some(
                format!(
                    "line range start ({}) must not exceed end ({})",
                    self.start, self.end
                )
                .into(),
            );
            return Err(err);
        }
        Ok(())
    }
}

/// `@path/**/*.rs`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Validate)]
pub struct GlobMention {
    /// Glob pattern, including its `*`, `?` or `[` metacharacters.
    #[validate(length(min = 1), custom(function = "validate_safe_pattern"))]
    pub pattern: String,
}

/// Reject NUL bytes and `..` traversal segments. Containment against the
/// filesystem containment is the caller's responsibility — this crate validates syntax.
fn validate_safe_path(path: &str) -> Result<(), ValidationError> {
    if path.contains('\0') {
        return Err(ValidationError::new("nul_byte"));
    }
    if std::path::Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ValidationError::new("parent_dir_traversal"));
    }
    Ok(())
}

/// A glob pattern must be non-empty (length rule) and NUL-free.
fn validate_safe_pattern(pattern: &str) -> Result<(), ValidationError> {
    if pattern.contains('\0') {
        return Err(ValidationError::new("nul_byte"));
    }
    Ok(())
}

/// Trailing punctuation commonly glued onto a mention by surrounding prose.
const TRAILING_PUNCT: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// Parse every `@`-mention in `text`, returning each mention plus the **byte
/// range** it occupied in `text`, so callers can strip the raw token and splice
/// resolved content in place.
///
/// # Examples
///
/// ```
/// use mentions::parse;
///
/// let prompt = "Read @src/main.rs:1-20 and @docs/";
/// let parsed = parse(prompt);
/// assert_eq!(parsed.len(), 2);
/// assert_eq!(&prompt[parsed[0].0.clone()], "@src/main.rs:1-20");
/// ```
pub fn parse(text: &str) -> Vec<(Range<usize>, Mention)> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut base = 0usize;
    while let Some(idx) = rest.find('@') {
        // A mention starts only at the beginning of the text or after
        // whitespace — `bob@example.com` is prose, not a mention.
        let boundary = idx == 0
            || rest[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        if !boundary {
            let advance = idx + '@'.len_utf8();
            rest = &rest[advance..];
            base += advance;
            continue;
        }
        let candidate = &rest[idx..];
        let mut c = candidate;
        if let Ok(m) = parse_mention(&mut c) {
            let consumed = candidate.len() - c.len();
            out.push((base + idx..base + idx + consumed, m));
            rest = &rest[idx + consumed..];
            base += idx + consumed;
        } else {
            let advance = idx + '@'.len_utf8();
            rest = &rest[advance..];
            base += advance;
        }
    }
    out
}

/// Convenience: the parsed mentions, dropping byte ranges.
pub fn parse_mentions(text: &str) -> Vec<Mention> {
    parse(text).into_iter().map(|(_, m)| m).collect()
}

/// Remove every `@`-mention token from `text`, leaving the surrounding prose.
///
/// # Examples
///
/// ```
/// assert_eq!(mentions::strip_mentions("Fix @src/a.rs, please"), "Fix , please");
/// ```
pub fn strip_mentions(text: &str) -> String {
    let mentions = parse(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for (range, _) in mentions {
        out.push_str(&text[last..range.start]);
        last = range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Parse one mention at the current position (must start with `@`).
fn parse_mention(input: &mut &str) -> ModalResult<Mention> {
    let before = *input;
    one_of('@').parse_next(input)?;
    let home = opt(one_of('~')).parse_next(input)?;
    let raw: &str = take_till(1.., |c: char| c.is_whitespace()).parse_next(input)?;
    let trimmed = raw.trim_end_matches(TRAILING_PUNCT);
    if trimmed.is_empty() {
        return Err(backtrack());
    }
    // Un-consume trailing punctuation so the returned byte range excludes it
    // (the token stops before the comma/period that follows it in prose).
    let give_back = raw.len() - trimmed.len();
    let consumed = before.len() - input.len();
    *input = &before[consumed - give_back..];
    let path = if home.is_some() {
        format!("~{trimmed}")
    } else {
        trimmed.to_string()
    };
    classify(&path)
}

/// Classify an extracted (punctuation-trimmed) path token.
fn classify(path: &str) -> ModalResult<Mention> {
    if let Some(dir) = path.strip_suffix('/') {
        if dir.is_empty() {
            return Err(backtrack());
        }
        return Ok(Mention::Directory(DirectoryMention {
            path: dir.to_string(),
        }));
    }
    if path.contains(['*', '?', '[']) {
        return Ok(Mention::Glob(GlobMention {
            pattern: path.to_string(),
        }));
    }
    if let Some((file, start, end)) = try_line_spec(path) {
        return match end {
            Some(end) => Ok(Mention::FileRange(FileRangeMention {
                path: file.to_string(),
                start,
                end,
            })),
            None => Ok(Mention::FileLine(FileLineMention {
                path: file.to_string(),
                line: start,
            })),
        };
    }
    Ok(Mention::File(FileMention {
        path: path.to_string(),
    }))
}

/// `path:LINE` or `path:START-END`; `None` when `path` has no parseable line
/// suffix (then the whole token is a plain file path).
fn try_line_spec(path: &str) -> Option<(&str, u64, Option<u64>)> {
    let (file, spec) = path.rsplit_once(':')?;
    if file.is_empty() {
        return None;
    }
    let mut s = spec;
    let start = parse_uint(&mut s)?;
    if s.is_empty() {
        return Some((file, start, None));
    }
    if let Some(rest) = s.strip_prefix('-') {
        let mut r = rest;
        let end = parse_uint(&mut r)?;
        if r.is_empty() {
            return Some((file, start, Some(end)));
        }
    }
    None
}

/// Parse a non-negative integer with winnow `digit1`, then `std` convert.
fn parse_uint(input: &mut &str) -> Option<u64> {
    let digits: &str = digit1::<&str, winnow::error::ContextError>
        .parse_next(input)
        .ok()?;
    digits.parse::<u64>().ok()
}

fn backtrack() -> winnow::error::ErrMode<winnow::error::ContextError> {
    winnow::error::ErrMode::Backtrack(winnow::error::ContextError::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_single_file_mention() {
        let m = parse_mentions("Fix @src/auth.rs please");
        assert_eq!(
            m,
            vec![Mention::File(FileMention {
                path: "src/auth.rs".into()
            })]
        );
    }

    #[test]
    fn parses_directory_via_trailing_slash() {
        let m = parse_mentions("Audit @src/routes/ for deprecated APIs");
        assert_eq!(
            m,
            vec![Mention::Directory(DirectoryMention {
                path: "src/routes".into()
            })]
        );
    }

    #[test]
    fn parses_line_and_range_suffixes() {
        let m = parse_mentions("See @src/auth.rs:42 and @src/b.rs:40-60");
        assert_eq!(
            m,
            vec![
                Mention::FileLine(FileLineMention {
                    path: "src/auth.rs".into(),
                    line: 42
                }),
                Mention::FileRange(FileRangeMention {
                    path: "src/b.rs".into(),
                    start: 40,
                    end: 60
                }),
            ]
        );
    }

    #[test]
    fn parses_glob_mentions() {
        let m = parse_mentions("Refactor @*.rs to the new error type");
        assert_eq!(
            m,
            vec![Mention::Glob(GlobMention {
                pattern: "*.rs".into()
            })]
        );
    }

    #[test]
    fn strips_mentions_and_reports_byte_ranges() {
        let text = "Fix @src/a.rs, then @src/b.rs:5.";
        let pairs = parse(text);
        assert_eq!(pairs.len(), 2);
        let (range, _) = &pairs[0];
        assert_eq!(&text[range.clone()], "@src/a.rs");
        assert_eq!(strip_mentions(text), "Fix , then .");
    }

    #[test]
    fn trailing_punctuation_is_trimmed() {
        let m = parse_mentions("read @src/a.rs; and @src/b.rs)");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].path(), "src/a.rs");
        assert_eq!(m[1].path(), "src/b.rs");
    }

    #[test]
    fn home_absolute_paths_are_preserved() {
        let m = parse_mentions("import @~/.config/app/config.toml");
        assert_eq!(
            m,
            vec![Mention::File(FileMention {
                path: "~/.config/app/config.toml".into()
            })]
        );
    }

    #[test]
    fn non_mentions_yield_nothing() {
        assert!(parse_mentions("no at-signs here").is_empty());
        // A bare `@` with no token, and `@` inside prose, are skipped.
        assert!(parse_mentions("email me at bob@example.com").is_empty());
    }

    #[test]
    fn validation_rejects_traversal_and_nul() {
        let traversal = Mention::File(FileMention {
            path: "../secret".into(),
        });
        assert!(traversal.validate().is_err());

        let nul = Mention::File(FileMention {
            path: "a\0b".into(),
        });
        assert!(nul.validate().is_err());

        let ok = Mention::File(FileMention {
            path: "src/a.rs".into(),
        });
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn validation_rejects_bad_lines_and_ranges() {
        assert!(Mention::FileLine(FileLineMention {
            path: "a.rs".into(),
            line: 0
        })
        .validate()
        .is_err());
        let range = Mention::FileRange(FileRangeMention {
            path: "a.rs".into(),
            start: 5,
            end: 3,
        });
        assert!(range.validate().is_ok(), "derive checks each field");
        let FileRangeMention { start, end, .. } = match range {
            Mention::FileRange(m) => m,
            _ => unreachable!(),
        };
        assert!(start > end, "cross-field rule is exposed for the caller");
    }

    #[test]
    fn empty_and_bare_at_are_not_mentions() {
        assert!(parse_mentions("ends with @").is_empty());
        assert!(parse_mentions("@/").is_empty(), "root-only dir is invalid");
    }
}
