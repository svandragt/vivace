//! PHP code emission for the autoloader files: `var_export`-style string
//! quoting, the array layout Composer writes into `autoload_static.php`, and
//! the `ClassLoader` property maps (`prefixLengthsPsr4` and friends) that
//! `ClassLoader::set`/`setPsr4` would build from the PSR maps.

/// A PHP value as `var_export` would print it. `Code` is emitted verbatim,
/// which is how path expressions such as `__DIR__ . '/..' . '/src'` get in.
#[derive(Debug, Clone)]
pub enum Php {
    Str(String),
    Int(usize),
    Code(String),
    /// Ordered, since PHP arrays are; keys are strings or integers.
    Arr(Vec<(Key, Php)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Int(usize),
    Str(String),
    /// A classmap class name: PHP strings are byte strings, and class names
    /// may contain bytes that are not valid UTF-8 (issue #71), so this key
    /// is exported raw rather than through the `String`-based `Str`.
    Bytes(Vec<u8>),
}

/// `var_export($s, true)`: single quotes, escaping `\` and `'`.
///
/// A path never needs escaping (no `\` or `'` in a Linux path this tool
/// writes), and this is called for every classmap/PSR-4 entry rendered, so
/// the common case skips straight to one allocation instead of the two
/// `String::replace` passes below, each of which allocates a fresh string
/// even when nothing matched.
pub fn export_str(s: &str) -> String {
    if !s.contains(['\\', '\'']) {
        return format!("'{s}'");
    }
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `var_export($s, true)` on raw bytes. Composer's `var_export` never
/// escapes bytes `>= 0x80` — it writes them unchanged inside the quotes, the
/// same as [`export_str`] but without requiring valid UTF-8.
pub fn export_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s {
        if b == b'\\' || b == b'\'' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b'\'');
    out
}

/// `var_export($value, true)` with PHP's two-space nested layout. Bytes, not
/// a `String`: a classmap key may embed non-UTF-8 bytes.
fn export(value: &Php, indent: usize, out: &mut Vec<u8>) {
    let pad = " ".repeat(indent);
    match value {
        Php::Str(s) => out.extend_from_slice(export_str(s).as_bytes()),
        Php::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Php::Code(code) => out.extend_from_slice(code.as_bytes()),
        Php::Arr(items) => {
            out.extend_from_slice(b"array (\n");
            for (key, item) in items {
                out.extend_from_slice(pad.as_bytes());
                out.extend_from_slice(b"  ");
                match key {
                    Key::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
                    Key::Str(s) => out.extend_from_slice(export_str(s).as_bytes()),
                    Key::Bytes(b) => out.extend_from_slice(&export_bytes(b)),
                }
                out.extend_from_slice(b" => ");
                if matches!(item, Php::Arr(_)) {
                    // PHP puts a nested array on its own line, leaving a
                    // trailing space after `=>` that Composer strips later.
                    out.push(b'\n');
                    out.extend_from_slice(pad.as_bytes());
                    out.extend_from_slice(b"  ");
                }
                export(item, indent + 2, out);
                out.extend_from_slice(b",\n");
            }
            out.extend_from_slice(pad.as_bytes());
            out.push(b')');
        }
    }
}

/// `var_export` output re-indented the way `AutoloadGenerator::getStaticFile`
/// does: every line gets four spaces plus its own indentation doubled, the
/// first line is left-trimmed and trailing spaces are removed.
///
/// Writes straight into one output buffer instead of collecting a `Vec<u8>`
/// per line and joining them: a classmap-heavy `-o` install re-indents
/// several thousand lines here, so the per-line allocation was showing up on
/// its own in a profile (#302).
pub fn export_static(value: &Php) -> Vec<u8> {
    let mut raw = Vec::new();
    export(value, 0, &mut raw);
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 8 + 16);
    for (i, line) in raw.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        let leading = line.iter().take_while(|&&b| b == b' ').count();
        let line_start = out.len();
        out.resize(line_start + 4 + leading, b' ');
        out.extend_from_slice(line);
        while out.len() > line_start && out.last().is_some_and(u8::is_ascii_whitespace) {
            out.pop();
        }
    }
    let start = out.iter().take_while(|b| b.is_ascii_whitespace()).count();
    out.drain(..start);
    out
}

/// The non-empty `ClassLoader` array properties, in declaration order, as
/// `ClassLoader::set` (PSR-0) and `ClassLoader::setPsr4` would populate them
/// from the `autoload_namespaces.php`/`autoload_psr4.php` maps. `psr0`/`psr4`
/// are `(prefix, path expressions)` in file order.
pub fn loader_properties(
    psr0: &[(String, Vec<Php>)],
    psr4: &[(String, Vec<Php>)],
) -> Vec<(&'static str, Php)> {
    let mut prefix_lengths: Vec<(Key, Php)> = Vec::new();
    let mut prefix_dirs: Vec<(Key, Php)> = Vec::new();
    let mut fallback_psr4: Vec<(Key, Php)> = Vec::new();
    for (prefix, paths) in psr4 {
        if prefix.is_empty() {
            fallback_psr4 = list(paths);
        } else {
            push_grouped(&mut prefix_lengths, prefix, Php::Int(prefix.len()));
            prefix_dirs.push((Key::Str(prefix.clone()), Php::Arr(list(paths))));
        }
    }

    let mut prefixes_psr0: Vec<(Key, Php)> = Vec::new();
    let mut fallback_psr0: Vec<(Key, Php)> = Vec::new();
    for (prefix, paths) in psr0 {
        if prefix.is_empty() {
            fallback_psr0 = list(paths);
        } else {
            push_grouped(&mut prefixes_psr0, prefix, Php::Arr(list(paths)));
        }
    }

    [
        ("prefixLengthsPsr4", prefix_lengths),
        ("prefixDirsPsr4", prefix_dirs),
        ("fallbackDirsPsr4", fallback_psr4),
        ("prefixesPsr0", prefixes_psr0),
        ("fallbackDirsPsr0", fallback_psr0),
    ]
    .into_iter()
    .filter(|(_, items)| !items.is_empty())
    .map(|(name, items)| (name, Php::Arr(items)))
    .collect()
}

fn list(paths: &[Php]) -> Vec<(Key, Php)> {
    paths
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, p)| (Key::Int(i), p))
        .collect()
}

/// `$map[$prefix[0]][$prefix] = $value`, keeping first-seen order of both
/// levels. `$prefix[0]` is the first byte, so multibyte prefixes group by
/// their lead byte exactly as PHP does.
fn push_grouped(groups: &mut Vec<(Key, Php)>, prefix: &str, value: Php) {
    let first = Key::Str(prefix[..1].to_string());
    let index = groups
        .iter()
        .position(|(key, _)| *key == first)
        .unwrap_or_else(|| {
            groups.push((first, Php::Arr(Vec::new())));
            groups.len() - 1
        });
    let group = &mut groups[index].1;
    if let Php::Arr(items) = group {
        let key = Key::Str(prefix.to_string());
        match items.iter().position(|(k, _)| *k == key) {
            Some(index) => items[index].1 = value,
            None => items.push((key, value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_str_escapes_backslash_and_quote() {
        assert_eq!(export_str("A\\B'C"), "'A\\\\B\\'C'");
    }

    #[test]
    fn static_layout_matches_composer_reindent() {
        let value = Php::Arr(vec![(
            Key::Str("S".into()),
            Php::Arr(vec![(Key::Str("Sit\\".into()), Php::Int(4))]),
        )]);
        assert_eq!(
            export_static(&value),
            b"array (\n        'S' =>\n        array (\n            'Sit\\\\' => 4,\n        ),\n    )"
                .to_vec()
        );
    }

    #[test]
    fn export_bytes_writes_high_bytes_raw() {
        // Composer's `var_export` never escapes bytes >= 0x80 (issue #71).
        assert_eq!(export_bytes(b"\xA9"), b"'\xA9'".to_vec());
    }
}
