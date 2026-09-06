//! Port of `composer/class-map-generator`: find PHP classes without loading
//! them, for `classmap` autoload entries.
//!
//! Two pieces, mirroring upstream: [`find_classes`] parses one file's bytes
//! (`PhpFileParser` + `PhpFileCleaner`), [`scan_paths`] walks a directory or
//! file (`ClassMapGenerator::scanPaths`, classmap mode only — no PSR-0/4
//! filtering, vivace does not need it).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use regex::bytes::Regex as BytesRegex;

/// Extensions Composer scans for classmap entries.
const EXTENSIONS: [&str; 3] = ["php", "inc", "hh"];

/// Result of scanning one or more paths: the class map plus any class found
/// in more than one file (first occurrence wins in `map`).
#[derive(Debug, Default)]
pub struct ClassMap {
    pub map: BTreeMap<String, PathBuf>,
    /// `(class, path already in the map, path this class was also found in)`.
    pub ambiguous: Vec<(String, PathBuf, PathBuf)>,
}

impl ClassMap {
    fn record(&mut self, class: String, path: &Path) {
        if let Some(existing) = self.map.get(&class) {
            if existing != path {
                self.ambiguous
                    .push((class, existing.clone(), path.to_path_buf()));
            }
        } else {
            self.map.insert(class, path.to_path_buf());
        }
    }
}

/// Fast bail-out: no `class`/`interface`/`trait`/`enum` keyword anywhere in
/// the raw source means cleaning cannot surface one either (cleaning only
/// removes or replaces text, never adds a keyword).
fn precheck_regex() -> &'static BytesRegex {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    RE.get_or_init(|| {
        BytesRegex::new(r"(?-u)(?i)\b(?:class|interface|trait|enum)\s")
            .expect("valid precheck regex")
    })
}

/// The class/interface/trait/enum declaration, or a `namespace` statement.
///
/// `(?-u)` because class names and namespace parts are matched byte-wise
/// (`\x7f-\xff`, as PCRE does without its `/u` modifier) — the `regex` crate
/// has no lookbehind, so the `(?<![\\$:>])` upstream uses is applied by hand
/// on the byte before each match instead.
fn token_regex() -> &'static BytesRegex {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    RE.get_or_init(|| {
        BytesRegex::new(
            r"(?ix-u)
            \b(?:
                 (?P<type>class|interface|trait|enum) \s+
                     (?P<name>[a-zA-Z_\x7f-\xff:][a-zA-Z0-9_\x7f-\xff:\-]*)
               | (?P<ns>namespace)
                     (?P<nsname>\s+[a-zA-Z_\x7f-\xff][a-zA-Z0-9_\x7f-\xff]*
                         (?:\s*\\\s*[a-zA-Z_\x7f-\xff][a-zA-Z0-9_\x7f-\xff]*)*
                     )?
                     \s*[{;]
            )",
        )
        .expect("valid token regex")
    })
}

/// Find the classes, interfaces, traits and enums declared in a PHP file.
///
/// Operates on raw bytes: fixtures include files that are not valid UTF-8
/// (in comments the parser strips), so class names are decoded lossily only
/// at the point they are emitted.
pub fn find_classes(source: &[u8]) -> Vec<String> {
    if !precheck_regex().is_match(source) {
        return Vec::new();
    }

    let cleaned = clean(source);
    let mut classes = Vec::new();
    let mut namespace = String::new();

    for caps in token_regex().captures_iter(&cleaned) {
        let whole = caps.get(0).expect("group 0 always matches");
        if whole.start() > 0 && matches!(cleaned[whole.start() - 1], b'\\' | b'$' | b':' | b'>') {
            // Rejected by the upstream `(?<![\\$:>])` lookbehind.
            continue;
        }

        if caps.name("ns").is_some() {
            namespace = match caps.name("nsname") {
                Some(nsname) => {
                    let stripped: Vec<u8> = nsname
                        .as_bytes()
                        .iter()
                        .copied()
                        .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                        .collect();
                    let mut ns = String::from_utf8_lossy(&stripped).into_owned();
                    ns.push('\\');
                    ns
                }
                // Braced global namespace: `namespace { ... }`.
                None => String::new(),
            };
            continue;
        }

        let name = caps.name("name").expect("type branch always names name");
        let mut name = String::from_utf8_lossy(name.as_bytes()).into_owned();
        // Anonymous classes: `new class extends Foo` / `new class implements Foo`.
        if name == "extends" || name == "implements" {
            continue;
        }

        if let Some(rest) = name.strip_prefix(':') {
            // XHP class, https://github.com/facebook/xhp
            let mut xhp = String::from("xhp");
            for c in rest.chars() {
                match c {
                    '-' => xhp.push('_'),
                    ':' => xhp.push_str("__"),
                    other => xhp.push(other),
                }
            }
            name = xhp;
        } else if caps
            .name("type")
            .is_some_and(|t| t.as_bytes().eq_ignore_ascii_case(b"enum"))
        {
            // `enum Foo: string { ... }` — the regex captures the colon and
            // backing type as part of the name; cut it back off.
            if let Some(colon) = name.rfind(':') {
                name.truncate(colon);
            }
        }

        classes.push(
            format!("{namespace}{name}")
                .trim_start_matches('\\')
                .to_string(),
        );
    }

    classes
}

/// Port of `PhpFileCleaner::clean`: drop anything outside `<?php ... ?>`,
/// replace comments with nothing and string/heredoc/nowdoc bodies with
/// `null`, so the token regex only ever sees real declarations.
///
/// Upstream also carries a single-match fast path keyed off a pre-count of
/// candidate keywords; that is a performance trick with no effect on the
/// result, so this port always runs the full scan.
fn clean(source: &[u8]) -> Vec<u8> {
    let len = source.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;

    'outer: while i < len {
        i = skip_to_php(source, i);
        out.extend_from_slice(b"<?");

        while i < len {
            let c = source[i];

            if c == b'?' && peek(source, i, b'>') {
                out.extend_from_slice(b"?>");
                i += 2;
                continue 'outer;
            }

            if c == b'"' || c == b'\'' {
                i = skip_string(source, i, c);
                out.extend_from_slice(b"null");
                continue;
            }

            if c == b'<'
                && peek(source, i, b'<')
                && let Some((after_start, delimiter)) = match_heredoc_start(source, i)
            {
                i = skip_heredoc(source, after_start, &delimiter);
                out.extend_from_slice(b"null");
                continue;
            }

            if c == b'/' {
                if peek(source, i, b'/') {
                    i = skip_to_newline(source, i);
                    continue;
                }
                if peek(source, i, b'*') {
                    i = skip_comment(source, i);
                    continue;
                }
            }

            out.push(c);
            i += 1;
        }
    }

    out
}

fn peek(source: &[u8], index: usize, want: u8) -> bool {
    index + 1 < source.len() && source[index + 1] == want
}

fn skip_to_php(source: &[u8], mut index: usize) -> usize {
    let len = source.len();
    while index < len {
        if source[index] == b'<' && peek(source, index, b'?') {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn skip_string(source: &[u8], mut index: usize, delimiter: u8) -> usize {
    let len = source.len();
    index += 1;
    while index < len {
        match source[index] {
            b'\\'
                if index + 1 < len
                    && (source[index + 1] == b'\\' || source[index + 1] == delimiter) =>
            {
                index += 2;
            }
            b'\\' => index += 1,
            c if c == delimiter => {
                index += 1;
                break;
            }
            _ => index += 1,
        }
    }
    index
}

fn skip_comment(source: &[u8], mut index: usize) -> usize {
    let len = source.len();
    index += 2;
    while index < len {
        if source[index] == b'*' && peek(source, index, b'/') {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn skip_to_newline(source: &[u8], mut index: usize) -> usize {
    let len = source.len();
    while index < len && source[index] != b'\r' && source[index] != b'\n' {
        index += 1;
    }
    index
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// Matches `<<<[ \t]*(['"]?)(ident)\1(?:\r\n|\n|\r)` anchored at `index`
/// (which points at the first `<`), returning the index just past the
/// opening line and the heredoc/nowdoc delimiter.
fn match_heredoc_start(source: &[u8], index: usize) -> Option<(usize, Vec<u8>)> {
    let len = source.len();
    if index + 3 > len || &source[index..index + 3] != b"<<<" {
        return None;
    }
    let mut j = index + 3;
    while j < len && (source[j] == b' ' || source[j] == b'\t') {
        j += 1;
    }
    let quote = matches!(source.get(j), Some(b'\'' | b'"')).then(|| {
        let q = source[j];
        j += 1;
        q
    });

    let start = j;
    if j >= len || !is_ident_start(source[j]) {
        return None;
    }
    j += 1;
    while j < len && is_ident_continue(source[j]) {
        j += 1;
    }
    let delimiter = source[start..j].to_vec();

    if let Some(q) = quote {
        if source.get(j) != Some(&q) {
            return None;
        }
        j += 1;
    }

    match source.get(j) {
        Some(b'\r') => {
            j += 1;
            if source.get(j) == Some(&b'\n') {
                j += 1;
            }
        }
        Some(b'\n') => j += 1,
        _ => return None,
    }

    Some((j, delimiter))
}

fn skip_heredoc(source: &[u8], mut index: usize, delimiter: &[u8]) -> usize {
    let len = source.len();
    let first = delimiter[0];
    loop {
        if index >= len {
            return index;
        }
        match source[index] {
            b'\t' | b' ' => {
                index += 1;
                continue;
            }
            c if c == first && source[index..].starts_with(delimiter) => {
                let after = index + delimiter.len();
                if !matches!(source.get(after), Some(&b) if is_ident_continue(b)) {
                    return after;
                }
            }
            _ => {}
        }

        index = skip_to_newline(source, index);
        while index < len && (source[index] == b'\r' || source[index] == b'\n') {
            index += 1;
        }
    }
}

/// Walk `path` (a file or a directory) for `.php`/`.inc`/`.hh` files and
/// collect the classes they declare. `exclude`, if given, matches against
/// each file's absolute, forward-slash path.
pub fn scan_paths(path: &Path, exclude: Option<&Regex>) -> Result<ClassMap> {
    let metadata = std::fs::metadata(path).map_err(|_| does_not_exist(path))?;

    let mut files = if metadata.is_file() {
        vec![path.to_path_buf()]
    } else if metadata.is_dir() {
        let mut files = Vec::new();
        walk_dir(path, &mut files)?;
        files.sort();
        files
    } else {
        return Err(does_not_exist(path));
    };

    // Extension filter applies even to a single explicit file, matching
    // Composer: a non-matching path is silently skipped, not an error.
    files.retain(|f| {
        f.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| EXTENSIONS.contains(&ext))
    });

    let mut class_map = ClassMap::default();
    let mut seen = HashSet::new();

    for file in files {
        let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }

        if let Some(exclude) = exclude {
            let absolute = canonical.to_string_lossy().replace('\\', "/");
            if exclude.is_match(&absolute) {
                continue;
            }
        }

        let source = std::fs::read(&file)?;
        for class in find_classes(&source) {
            class_map.record(class, &file);
        }
    }

    Ok(class_map)
}

fn does_not_exist(path: &Path) -> anyhow::Error {
    anyhow!(
        "Could not scan for classes inside \"{}\" which does not appear to be a file nor a folder",
        path.display()
    )
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // `metadata` (not `symlink_metadata`) follows symlinks, matching
        // Symfony Finder's `followLinks()`.
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            walk_dir(&path, out)?;
        } else if metadata.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_strings_and_finds_class() {
        let source = b"<?php\n// class Ignored\n/* class AlsoIgnored */\n$s = 'class StillIgnored';\nclass Real {}\n";
        assert_eq!(find_classes(source), vec!["Real".to_string()]);
    }

    #[test]
    fn rejects_class_after_double_colon_dollar_arrow() {
        let source = b"<?php\n$x = Foo::class;\n$class = 1;\n$y->class;\nclass Real {}\n";
        assert_eq!(find_classes(source), vec!["Real".to_string()]);
    }

    #[test]
    fn namespace_prefixes_following_classes() {
        let source = b"<?php\nnamespace Foo\\Bar;\nclass Baz {}\n";
        assert_eq!(find_classes(source), vec!["Foo\\Bar\\Baz".to_string()]);
    }

    #[test]
    fn enum_backing_type_is_cut_from_name() {
        let source = b"<?php\nenum Foo: string implements Bar {\n}\n";
        assert_eq!(find_classes(source), vec!["Foo".to_string()]);
    }

    #[test]
    fn anonymous_class_extends_or_implements_is_skipped() {
        let source =
            b"<?php\nnew class extends Foo {};\nnew class implements Bar {};\nclass Real {}\n";
        assert_eq!(find_classes(source), vec!["Real".to_string()]);
    }
}
