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
}

/// `var_export($s, true)`: single quotes, escaping `\` and `'`.
pub fn export_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `var_export($value, true)` with PHP's two-space nested layout.
fn export(value: &Php, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Php::Str(s) => out.push_str(&export_str(s)),
        Php::Int(n) => out.push_str(&n.to_string()),
        Php::Code(code) => out.push_str(code),
        Php::Arr(items) => {
            out.push_str("array (\n");
            for (key, item) in items {
                out.push_str(&pad);
                out.push_str("  ");
                match key {
                    Key::Int(n) => out.push_str(&n.to_string()),
                    Key::Str(s) => out.push_str(&export_str(s)),
                }
                out.push_str(" => ");
                if matches!(item, Php::Arr(_)) {
                    // PHP puts a nested array on its own line, leaving a
                    // trailing space after `=>` that Composer strips later.
                    out.push('\n');
                    out.push_str(&pad);
                    out.push_str("  ");
                }
                export(item, indent + 2, out);
                out.push_str(",\n");
            }
            out.push_str(&pad);
            out.push(')');
        }
    }
}

/// `var_export` output re-indented the way `AutoloadGenerator::getStaticFile`
/// does: every line gets four spaces plus its own indentation doubled, the
/// first line is left-trimmed and trailing spaces are removed.
pub fn export_static(value: &Php) -> String {
    let mut raw = String::new();
    export(value, 0, &mut raw);
    let lines: Vec<String> = raw
        .lines()
        .map(|line| {
            let leading = line.len() - line.trim_start_matches(' ').len();
            format!("    {}{}", " ".repeat(leading), line)
                .trim_end()
                .to_string()
        })
        .collect();
    lines.join("\n").trim_start().to_string()
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
            "array (\n        'S' =>\n        array (\n            'Sit\\\\' => 4,\n        ),\n    )"
        );
    }
}
