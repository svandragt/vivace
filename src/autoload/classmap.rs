//! Port of `composer/class-map-generator`: find PHP classes without loading
//! them, for `classmap` autoload entries.
//!
//! Two pieces, mirroring upstream: [`find_classes`] parses one file's bytes
//! (`PhpFileParser` + `PhpFileCleaner`), [`scan_paths`] walks a directory or
//! file (`ClassMapGenerator::scanPaths`, classmap mode only — no PSR-0/4
//! filtering, vivace does not need it).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use regex::bytes::Regex as BytesRegex;
use serde::{Deserialize, Serialize};

/// Extensions Composer scans for classmap entries.
const EXTENSIONS: [&str; 3] = ["php", "inc", "hh"];
const VCS_DIRS: [&str; 8] = [
    ".svn",
    "_svn",
    "CVS",
    "_darcs",
    ".arch-params",
    ".monotone",
    ".bzr",
    ".hg",
];

/// A class/interface/trait/enum name as Composer keeps it: the raw bytes a
/// PHP source declared, not necessarily valid UTF-8 (`class \xA9 {}` is
/// legal PHP — identifiers only need to avoid ASCII punctuation). `BTreeMap`
/// orders `Vec<u8>` byte-wise, which is exactly what PHP's `ksort` does.
pub type ClassName = Vec<u8>;

/// Result of scanning one or more paths: the class map plus any class found
/// in more than one file (first occurrence wins in `map`).
#[derive(Debug, Default)]
pub struct ClassMap {
    pub map: BTreeMap<ClassName, PathBuf>,
    /// `(class, path already in the map, path this class was also found in)`.
    pub ambiguous: Vec<(ClassName, PathBuf, PathBuf)>,
    /// Every scanned file's canonical path, keyed by its literal path — the
    /// canonicalize this module already did to dedupe symlinked duplicates,
    /// reused by callers (`generator::Scanner`) instead of canonicalizing
    /// the same file a second time.
    pub canonical: HashMap<PathBuf, PathBuf>,
}

impl ClassMap {
    fn record(&mut self, class: ClassName, path: &Path) {
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
/// Operates on raw bytes throughout, never decoding to UTF-8: PHP identifiers
/// may contain any byte `>= 0x80` (`class \xA9 {}` is legal), and Composer's
/// classmap keeps whatever bytes the source declared.
pub fn find_classes(source: &[u8]) -> Vec<ClassName> {
    if !precheck_regex().is_match(source) {
        return Vec::new();
    }

    let cleaned = clean(source);
    let mut classes = Vec::new();
    let mut namespace: Vec<u8> = Vec::new();

    for caps in token_regex().captures_iter(&cleaned) {
        let whole = caps.get(0).expect("group 0 always matches");
        if whole.start() > 0 && matches!(cleaned[whole.start() - 1], b'\\' | b'$' | b':' | b'>') {
            // Rejected by the upstream `(?<![\\$:>])` lookbehind.
            continue;
        }

        if caps.name("ns").is_some() {
            namespace = match caps.name("nsname") {
                Some(nsname) => {
                    let mut ns: Vec<u8> = nsname
                        .as_bytes()
                        .iter()
                        .copied()
                        .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                        .collect();
                    ns.push(b'\\');
                    ns
                }
                // Braced global namespace: `namespace { ... }`.
                None => Vec::new(),
            };
            continue;
        }

        let name = caps.name("name").expect("type branch always names name");
        let name = name.as_bytes();
        // Anonymous classes: `new class extends Foo` / `new class implements Foo`.
        if name == b"extends" || name == b"implements" {
            continue;
        }

        let name: Vec<u8> = if let Some(rest) = name.strip_prefix(b":") {
            // XHP class, https://github.com/facebook/xhp
            let mut xhp = b"xhp".to_vec();
            for &b in rest {
                match b {
                    b'-' => xhp.push(b'_'),
                    b':' => xhp.extend_from_slice(b"__"),
                    other => xhp.push(other),
                }
            }
            xhp
        } else if caps
            .name("type")
            .is_some_and(|t| t.as_bytes().eq_ignore_ascii_case(b"enum"))
        {
            // `enum Foo: string { ... }` — the regex captures the colon and
            // backing type as part of the name; cut it back off.
            match name.iter().rposition(|&b| b == b':') {
                Some(colon) => name[..colon].to_vec(),
                None => name.to_vec(),
            }
        } else {
            name.to_vec()
        };

        let mut class = namespace.clone();
        class.extend_from_slice(&name);
        let start = class.iter().take_while(|&&b| b == b'\\').count();
        classes.push(class[start..].to_vec());
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
        walk_dir(path, &mut HashSet::new(), &mut files)?;
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
        // Composer scans a file reachable through two symlinks twice and
        // reports it ambiguous with itself. vivace dedups on the canonical
        // path instead: the output is deterministic and a file cannot be
        // ambiguous with itself.
        let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }
        class_map.canonical.insert(file.clone(), canonical.clone());

        if let Some(exclude) = exclude {
            // Both the realpath and the literal path, as upstream does, so a
            // symlinked directory can be excluded by its project path.
            let literal = std::path::absolute(&file).unwrap_or_else(|_| file.clone());
            let excluded = [canonical.as_path(), literal.as_path()]
                .iter()
                .any(|p| exclude.is_match(&p.to_string_lossy().replace('\\', "/")));
            if excluded {
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

/// Scan parameters that affect a [`ClassMap`] result beyond an archive's own
/// (immutable) content: matched against a cached result before it's reused,
/// so a change to any of them is a cache miss rather than a wrong classmap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanKey {
    /// The scanned directory's path relative to the archive root (empty for
    /// the package root itself). For the root-package cache (#269), which
    /// has no archive root to be relative to, this is just an opaque
    /// identity for the directory within that project's one sidecar file —
    /// [`Fingerprint`] is what actually stands in for its content.
    pub subpath: String,
    /// The exclusion regex's own source, if any was built for this scan.
    pub exclude: Option<String>,
    /// `(namespace, "psr-0"|"psr-4")` for a PSR scan, `None` for a plain
    /// `classmap` entry.
    pub psr: Option<(String, String)>,
    /// #269: the root package's own directories have no store archive (they
    /// are not immutable, so nothing content-addresses them) — a fingerprint
    /// standing in for "unchanged since this was cached" instead. `None` for
    /// a store-archive scan, where the archive dir itself already is that
    /// guarantee. `#[serde(default)]` so a sidecar written before this field
    /// existed still parses (as a `None` on every entry, a well-formed cache
    /// miss rather than a corrupt-file wipe of the whole sidecar).
    #[serde(default)]
    pub fingerprint: Option<Fingerprint>,
}

/// A cheap stand-in for a directory's content: the recursive max mtime across
/// every directory and file under it, plus a file count as a second check —
/// two operations within the same mtime tick (coarse filesystem resolution,
/// or just fast enough hardware) would otherwise look unchanged. Cost is a
/// `stat` per entry (`readdir` plus metadata), no file content read, so it's
/// far cheaper than the [`scan_paths`] walk it guards, but still linear in
/// the directory's own size — a directory that changes on every run gains
/// nothing from this and pays the fingerprint walk on top of the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    max_mtime: (i64, u32),
    file_count: u64,
}

/// Compute [`Fingerprint`] for `dir`, walking it the same way [`scan_paths`]
/// does (dot files and VCS dirs skipped, symlink cycles broken) so the two
/// never disagree about what's "under" the directory.
pub fn fingerprint_dir(dir: &Path) -> Result<Fingerprint> {
    let mut visited = HashSet::new();
    let mut fp = Fingerprint {
        max_mtime: (0, 0),
        file_count: 0,
    };
    fingerprint_walk(dir, &mut visited, &mut fp)?;
    Ok(fp)
}

fn fingerprint_walk(
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    fp: &mut Fingerprint,
) -> Result<()> {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(());
    }
    bump_mtime(fp, dir);
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || VCS_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if metadata.is_dir() {
            fingerprint_walk(&path, visited, fp)?;
        } else if metadata.is_file() {
            fp.file_count += 1;
            bump_mtime_of(fp, &metadata);
        }
    }
    Ok(())
}

fn bump_mtime(fp: &mut Fingerprint, path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        bump_mtime_of(fp, &metadata);
    }
}

fn bump_mtime_of(fp: &mut Fingerprint, metadata: &std::fs::Metadata) {
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return;
    };
    let stamp = (
        i64::try_from(since_epoch.as_secs()).unwrap_or(0),
        since_epoch.subsec_nanos(),
    );
    if stamp > fp.max_mtime {
        fp.max_mtime = stamp;
    }
}

/// The root package's classmap-scan sidecar (#269's `root-classmap-v0`), as
/// written: paths relative to the scanned root, so a cached scan applies
/// wherever that root is linked next (a rebuilt `vendor/`, or another
/// project's) — the `canonical` map only earns its keep mid-scan, deduping
/// symlinks a cache hit never walks.
///
/// A `Vec` of entries, not one: a package (root or archive) commonly gets
/// scanned under more than one [`ScanKey`] — several classmap directories, a
/// PSR-4 namespace mapped onto more than one directory (`vendor/symfony/
/// polyfill-*`'s base dir plus its `Resources/stubs`), or both a classmap and
/// a PSR-4 rule over the same subpath (`nette/schema`). #77: storing only the
/// latest key made every other one thrash — evicted and rescanned on every
/// single warm run, not once.
///
/// #303 moved the *archive* sidecar to [`SidecarV1`]'s already-merge-shaped
/// format instead; this raw-scan/JSON shape stays for the root package only,
/// because a namespace-mismatch warning for the root's own files is real
/// (unlike a vendor package's, always suppressed — see
/// `generator::Scanner::shape_for_merge`) and must keep firing on a cache hit
/// exactly as it does on a miss, which only works if PSR-4 filtering is still
/// deferred to merge time rather than baked into the cached bytes.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Sidecar(Vec<CachedScan>);

#[derive(Debug, Serialize, Deserialize)]
struct CachedScan {
    key: ScanKey,
    /// `(hex-encoded class name, path relative to the scanned root)`: a JSON
    /// object needs string keys, and a class name is raw bytes.
    classes: Vec<(String, PathBuf)>,
    ambiguous: Vec<(String, PathBuf, PathBuf)>,
}

fn to_hex(class: &[u8]) -> String {
    crate::store::hex(class)
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then_some(())?;
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

impl CachedScan {
    fn from_found(key: ScanKey, dir: &Path, found: &ClassMap) -> Self {
        let relative = |p: &Path| p.strip_prefix(dir).unwrap_or(p).to_path_buf();
        CachedScan {
            key,
            classes: found
                .map
                .iter()
                .map(|(class, path)| (to_hex(class), relative(path)))
                .collect(),
            ambiguous: found
                .ambiguous
                .iter()
                .map(|(class, a, b)| (to_hex(class), relative(a), relative(b)))
                .collect(),
        }
    }

    /// Re-root this entry's paths onto `dir`, or `None` for corrupt hex (a
    /// hand-edited or truncated sidecar) — a cache miss, not an error.
    fn to_class_map(&self, dir: &Path) -> Option<ClassMap> {
        let mut class_map = ClassMap::default();
        for (class, path) in &self.classes {
            class_map.map.insert(from_hex(class)?, dir.join(path));
        }
        for (class, a, b) in &self.ambiguous {
            class_map
                .ambiguous
                .push((from_hex(class)?, dir.join(a), dir.join(b)));
        }
        Some(class_map)
    }
}

impl Sidecar {
    /// Read every cached scan `sidecar` holds, once. A missing file or
    /// corrupt content is an empty sidecar (a miss on every key), not an
    /// error: the caller always has [`scan_paths`] to fall back to.
    pub(crate) fn read(sidecar: &Path) -> Self {
        fs_err::read(sidecar)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(crate) fn get(&self, key: &ScanKey, dir: &Path) -> Option<ClassMap> {
        self.0.iter().find(|e| e.key == *key)?.to_class_map(dir)
    }

    /// Replace or add `key`'s entry and persist the whole sidecar again:
    /// every other key this archive already had stays cached, so a later
    /// scan of a different subpath in the same archive is still a hit
    /// rather than evicting and rewriting on every run (#77).
    ///
    /// Temp file plus rename, like [`crate::store`]'s `.ok` marker, so a
    /// concurrent reader never observes a partial write.
    pub(crate) fn insert_and_write(
        &mut self,
        sidecar: &Path,
        key: &ScanKey,
        dir: &Path,
        found: &ClassMap,
    ) -> Result<()> {
        let entry = CachedScan::from_found(key.clone(), dir, found);
        match self.0.iter_mut().find(|e| e.key == *key) {
            Some(existing) => *existing = entry,
            None => self.0.push(entry),
        }
        let parent = sidecar
            .parent()
            .expect("sidecar is nested under the archive dir");
        // A no-op for an archive sidecar (the archive dir already made
        // `parent`); needed for the root-package sidecar's own bucket
        // (#269), which nothing else creates.
        fs_err::create_dir_all(parent)?;
        let mut temp = tempfile::Builder::new()
            .prefix(".tmp-classmap-")
            .tempfile_in(parent)?;
        serde_json::to_writer(&mut temp, &self.0)?;
        temp.persist(sidecar)?;
        Ok(())
    }
}

/// Read a cached scan from `sidecar` for `key`, re-rooting its relative
/// paths onto `dir`. A missing file, corrupt content, or no entry matching
/// `key` is a cache miss, not an error: the caller always has
/// [`scan_paths`] to fall back to. A one-shot convenience over `Sidecar`
/// for callers (mainly tests) that don't need to reuse the parsed sidecar
/// across more than one key; `Sidecar::read`/`Sidecar::get` do that.
pub fn read_cached_scan(sidecar: &Path, key: &ScanKey, dir: &Path) -> Option<ClassMap> {
    Sidecar::read(sidecar).get(key, dir)
}

/// Write a fresh [`scan_paths`] result for `dir` under `key` into `sidecar`,
/// alongside whatever other keys that archive was already cached under.
/// Best-effort like `Sidecar::insert_and_write`; see its doc for the
/// merge-not-overwrite rationale.
pub fn write_cached_scan(
    sidecar: &Path,
    key: &ScanKey,
    dir: &Path,
    found: &ClassMap,
) -> Result<()> {
    let mut cache = Sidecar::read(sidecar);
    cache.insert_and_write(sidecar, key, dir, found)
}

// ---------------------------------------------------------------------
// #303: the per-archive sidecar, already in the merge's own final shape.
// ---------------------------------------------------------------------

/// A store archive's classmap-scan sidecar (`.classmap-v1`,
/// `store::archive_classmap_sidecar`): unlike [`Sidecar`]'s raw per-class
/// scan result, each entry here is already what `generator::Scanner`'s fold
/// used to re-derive from that raw result on every single run — files
/// grouped (a class found in two files under one scan lists both, same as
/// [`Sidecar`]'s `ambiguous` did, so the merge's first-wins tie-break can
/// still choose between them), sorted into the final lexicographic-by-file
/// order that tie-break depends on (#198, #259), and PSR-4 filtering already
/// applied — a namespace mismatch is deterministic for a given archive/key,
/// so a hit has nothing left to recompute. Paths are relative to the archive
/// root (`ScanKey::archive_relative`), not to whichever subdirectory a
/// particular key scanned, so every key sharing one archive's sidecar spells
/// a path from the same base.
///
/// A pre-#303 `.classmap-v0` sidecar is never read as a compatibility
/// fallback: a miss is cheaper than carrying two decoders, and a miss here
/// just rewrites a fresh `.classmap-v1` file next to it.
#[derive(Debug, Default)]
pub(crate) struct SidecarV1(Vec<CachedScanV1>);

#[derive(Debug)]
struct CachedScanV1 {
    key: ScanKey,
    /// `(path relative to the archive root, classes)`, already in final
    /// lexicographic-by-path order and already PSR-4 filtered.
    files: Vec<(PathBuf, Vec<ClassName>)>,
}

impl ScanKey {
    /// A file's path relative to the directory this key scanned, as stored
    /// in a [`SidecarV1`] entry: relative to the archive root instead, by
    /// prepending this key's own `subpath`.
    fn archive_relative(&self, rel: &Path) -> PathBuf {
        if self.subpath.is_empty() {
            rel.to_path_buf()
        } else {
            Path::new(&self.subpath).join(rel)
        }
    }

    /// The inverse of [`Self::archive_relative`]: strip this key's own
    /// `subpath` back off, so the result is relative to the directory the
    /// key actually scanned again.
    fn strip_archive_prefix<'a>(&self, root_relative: &'a Path) -> &'a Path {
        if self.subpath.is_empty() {
            root_relative
        } else {
            root_relative
                .strip_prefix(&self.subpath)
                .unwrap_or(root_relative)
        }
    }
}

impl SidecarV1 {
    /// Read every cached scan `sidecar` holds, once. A missing file, a
    /// `.classmap-v0` file (wrong shape and encoding — its length-prefixed
    /// bytes are read as v1's, which either fails a bounds check or decodes
    /// as garbage, both treated as corrupt), or genuinely corrupt content is
    /// an empty sidecar (a miss on every key), not an error.
    pub(crate) fn read(sidecar: &Path) -> Self {
        fs_err::read(sidecar)
            .ok()
            .and_then(|bytes| decode(&bytes))
            .unwrap_or_default()
    }

    /// Re-root `key`'s entry onto `dir` (the directory this key scans), or
    /// `None` on a miss — no matching key, same "cache miss, not an error"
    /// contract as [`Sidecar::get`].
    pub(crate) fn get(&self, key: &ScanKey, dir: &Path) -> Option<Vec<(PathBuf, Vec<ClassName>)>> {
        let entry = self.0.iter().find(|e| e.key == *key)?;
        Some(
            entry
                .files
                .iter()
                .map(|(root_relative, classes)| {
                    (
                        dir.join(key.strip_archive_prefix(root_relative)),
                        classes.clone(),
                    )
                })
                .collect(),
        )
    }

    /// Replace or add `key`'s entry and persist the whole sidecar again —
    /// merge-not-overwrite, same rationale as [`Sidecar::insert_and_write`]
    /// (#77). `shaped` is already final: absolute paths under `dir`, grouped,
    /// ordered and PSR-4 filtered by the caller (`generator::Scanner::
    /// shape_for_merge`) — this only re-roots them onto the archive and
    /// encodes.
    pub(crate) fn insert_and_write(
        &mut self,
        sidecar: &Path,
        key: &ScanKey,
        dir: &Path,
        shaped: &[(PathBuf, Vec<ClassName>)],
    ) -> Result<()> {
        let files = shaped
            .iter()
            .map(|(file, classes)| {
                let rel = file.strip_prefix(dir).unwrap_or(file);
                (key.archive_relative(rel), classes.clone())
            })
            .collect();
        let entry = CachedScanV1 {
            key: key.clone(),
            files,
        };
        match self.0.iter_mut().find(|e| e.key == *key) {
            Some(existing) => *existing = entry,
            None => self.0.push(entry),
        }
        let parent = sidecar
            .parent()
            .expect("sidecar is nested under the archive dir");
        fs_err::create_dir_all(parent)?;
        let mut temp = tempfile::Builder::new()
            .prefix(".tmp-classmap-")
            .tempfile_in(parent)?;
        std::io::Write::write_all(&mut temp, &encode(&self.0)?)?;
        temp.persist(sidecar)?;
        Ok(())
    }
}

/// Length-prefixed binary encoding: no serde-compatible binary crate is in
/// the dependency tree (`Cargo.toml` has `serde_json` only), and adding one
/// for this alone would cost more than it saves over a hand-written layout
/// this small. Every count and length is a little-endian `u32`; a class name
/// is raw bytes (no hex, unlike [`Sidecar`]'s JSON, which needs a string
/// key), a path is UTF-8 (already assumed throughout this codebase, which is
/// Linux-only — see the module doc).
fn encode(entries: &[CachedScanV1]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_u32(&mut buf, len_u32(entries.len()));
    for entry in entries {
        write_key(&mut buf, &entry.key);
        write_u32(&mut buf, len_u32(entry.files.len()));
        for (path, classes) in &entry.files {
            let path = path.to_str().ok_or_else(|| {
                anyhow!(
                    "classmap sidecar path is not valid UTF-8: {}",
                    path.display()
                )
            })?;
            write_bytes(&mut buf, path.as_bytes());
            write_u32(&mut buf, len_u32(classes.len()));
            for class in classes {
                write_bytes(&mut buf, class);
            }
        }
    }
    Ok(buf)
}

/// A count or length as a `u32` for the sidecar's own on-disk format: every
/// value this is called on (archives, files, classes, path bytes) is well
/// under 2^32 for any real project, so a value that somehow isn't just
/// clamps rather than threading a `Result` through the whole encoder for an
/// input that never occurs.
fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn decode(bytes: &[u8]) -> Option<SidecarV1> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let count = r.read_u32()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let key = read_key(&mut r)?;
        let file_count = r.read_u32()?;
        let mut files = Vec::new();
        for _ in 0..file_count {
            let path = PathBuf::from(r.read_str()?);
            let class_count = r.read_u32()?;
            let mut classes = Vec::new();
            for _ in 0..class_count {
                classes.push(r.read_bytes()?.to_vec());
            }
            files.push((path, classes));
        }
        entries.push(CachedScanV1 { key, files });
    }
    Some(SidecarV1(entries))
}

fn write_key(buf: &mut Vec<u8>, key: &ScanKey) {
    write_bytes(buf, key.subpath.as_bytes());
    match &key.exclude {
        Some(exclude) => {
            buf.push(1);
            write_bytes(buf, exclude.as_bytes());
        }
        None => buf.push(0),
    }
    match &key.psr {
        Some((ns, kind)) => {
            buf.push(1);
            write_bytes(buf, ns.as_bytes());
            write_bytes(buf, kind.as_bytes());
        }
        None => buf.push(0),
    }
    match &key.fingerprint {
        // Always `None` for an archive scan (see `ScanKey::fingerprint`'s
        // own doc) but encoded anyway rather than forking the format: one
        // `ScanKey` shape, one encoder, whichever sidecar it ends up in.
        Some(fp) => {
            buf.push(1);
            buf.extend_from_slice(&fp.max_mtime.0.to_le_bytes());
            buf.extend_from_slice(&fp.max_mtime.1.to_le_bytes());
            buf.extend_from_slice(&fp.file_count.to_le_bytes());
        }
        None => buf.push(0),
    }
}

fn read_key(r: &mut Reader<'_>) -> Option<ScanKey> {
    let subpath = r.read_str()?;
    let exclude = match r.read_u8()? {
        0 => None,
        _ => Some(r.read_str()?),
    };
    let psr = match r.read_u8()? {
        0 => None,
        _ => Some((r.read_str()?, r.read_str()?)),
    };
    let fingerprint = match r.read_u8()? {
        0 => None,
        _ => Some(Fingerprint {
            max_mtime: (r.read_i64()?, r.read_u32()?),
            file_count: r.read_u64()?,
        }),
    };
    Some(ScanKey {
        subpath,
        exclude,
        psr,
        fingerprint,
    })
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    write_u32(buf, len_u32(bytes.len()));
    buf.extend_from_slice(bytes);
}

/// A cursor over a sidecar's bytes: every read is bounds-checked against
/// what's actually left in `buf`, so a truncated file (or a `.classmap-v0`
/// JSON file misread as v1) yields `None` at the first short read instead of
/// panicking or over-allocating from an untrusted length.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn read_u32(&mut self) -> Option<u32> {
        let bytes = self.buf.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    fn read_i64(&mut self) -> Option<i64> {
        let bytes = self.buf.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(i64::from_le_bytes(bytes.try_into().ok()?))
    }

    fn read_u64(&mut self) -> Option<u64> {
        let bytes = self.buf.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    fn read_bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.read_u32()? as usize;
        let bytes = self.buf.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some(bytes)
    }

    fn read_str(&mut self) -> Option<String> {
        String::from_utf8(self.read_bytes()?.to_vec()).ok()
    }
}

fn does_not_exist(path: &Path) -> anyhow::Error {
    anyhow!(
        "Could not scan for classes inside \"{}\" which does not appear to be a file nor a folder",
        path.display()
    )
}

/// `visited` holds canonical directory paths so a symlink cycle
/// (`a/b/self -> ../..`) terminates instead of recursing until ELOOP.
fn walk_dir(dir: &Path, visited: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) -> Result<()> {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // `metadata` (not `symlink_metadata`) follows symlinks, matching
        // Symfony Finder's `followLinks()`. A broken link or unreadable
        // entry is skipped, not fatal: one stray symlink must not abort
        // the whole install.
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                tracing::warn!("skipping {}: {err}", path.display());
                continue;
            }
        };
        // Symfony Finder's defaults: ignoreDotFiles(true) and
        // ignoreVCS(true). Applies below the scanned root only, so a root
        // like `.hidden/` passed explicitly to `scan_paths` is still walked.
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || VCS_DIRS.contains(&name.as_ref()) {
            continue;
        }

        if metadata.is_dir() {
            walk_dir(&path, visited, out)?;
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
        assert_eq!(find_classes(source), vec![b"Real".to_vec()]);
    }

    #[test]
    fn rejects_class_after_double_colon_dollar_arrow() {
        let source = b"<?php\n$x = Foo::class;\n$class = 1;\n$y->class;\nclass Real {}\n";
        assert_eq!(find_classes(source), vec![b"Real".to_vec()]);
    }

    #[test]
    fn namespace_prefixes_following_classes() {
        let source = b"<?php\nnamespace Foo\\Bar;\nclass Baz {}\n";
        assert_eq!(find_classes(source), vec![b"Foo\\Bar\\Baz".to_vec()]);
    }

    #[test]
    fn enum_backing_type_is_cut_from_name() {
        let source = b"<?php\nenum Foo: string implements Bar {\n}\n";
        assert_eq!(find_classes(source), vec![b"Foo".to_vec()]);
    }

    #[test]
    fn anonymous_class_extends_or_implements_is_skipped() {
        let source =
            b"<?php\nnew class extends Foo {};\nnew class implements Bar {};\nclass Real {}\n";
        assert_eq!(find_classes(source), vec![b"Real".to_vec()]);
    }

    #[test]
    fn class_name_keeps_non_utf8_byte() {
        // `\xA9` is a valid PHP identifier byte (`\x7f-\xff`) but not valid
        // UTF-8 on its own; Composer's classmap keeps it raw (issue #71).
        let source = b"<?php\nclass \xA9 {}\n";
        assert_eq!(find_classes(source), vec![b"\xA9".to_vec()]);
    }

    #[test]
    fn sidecar_v1_round_trips_files_in_whatever_order_theyre_given() {
        // `SidecarV1` is a dumb transport: the "final lexicographic order"
        // guarantee is the caller's (`generator::Scanner::shape_for_merge`),
        // proven here by writing two files out of order and getting the same
        // order straight back, not re-sorted underneath the caller.
        let dir = tempfile::tempdir().expect("tempdir");
        let scanned = dir.path().join("scanned");
        std::fs::create_dir_all(&scanned).unwrap();
        let sidecar = dir.path().join("archive").with_extension("classmap-v1");

        let key = ScanKey {
            subpath: "src".to_string(),
            exclude: None,
            psr: None,
            fingerprint: None,
        };
        let shaped = vec![
            (scanned.join("b.php"), vec![b"B".to_vec()]),
            (scanned.join("a.php"), vec![b"A".to_vec(), b"A2".to_vec()]),
        ];
        let mut cache = SidecarV1::default();
        cache
            .insert_and_write(&sidecar, &key, &scanned, &shaped)
            .expect("write should succeed");

        let hit = SidecarV1::read(&sidecar)
            .get(&key, &scanned)
            .expect("a matching key should hit");
        assert_eq!(hit, shaped);
    }

    #[test]
    fn sidecar_v1_misses_on_a_different_key_or_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sidecar = dir.path().join("archive").with_extension("classmap-v1");
        let key = ScanKey {
            subpath: String::new(),
            exclude: None,
            psr: None,
            fingerprint: None,
        };
        assert!(
            SidecarV1::read(&sidecar).get(&key, dir.path()).is_none(),
            "no sidecar file at all is a miss"
        );

        let mut cache = SidecarV1::default();
        cache
            .insert_and_write(&sidecar, &key, dir.path(), &[])
            .expect("write should succeed");
        let different = ScanKey {
            psr: Some(("App\\".to_string(), "psr-4".to_string())),
            ..key
        };
        assert!(
            SidecarV1::read(&sidecar)
                .get(&different, dir.path())
                .is_none()
        );
    }

    /// #303's own done-when: an older viv's `.classmap-v0` sidecar next to an
    /// archive is never read as v1 (a different file entirely), so it's a
    /// miss regardless of what it holds; the miss then writes a fresh
    /// `.classmap-v1` file, leaving the stale one untouched rather than
    /// upgrading it in place.
    #[test]
    fn v1_ignores_a_stale_v0_sidecar_and_a_miss_writes_v1() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("Foo.php"), "<?php\nclass Foo {}\n").unwrap();

        let key = ScanKey {
            subpath: String::new(),
            exclude: None,
            psr: None,
            fingerprint: None,
        };
        let found = scan_paths(&archive, None).expect("scan should succeed");
        let v0 = archive.with_extension("classmap-v0");
        write_cached_scan(&v0, &key, &archive, &found).expect("v0 write should succeed");

        let v1 = crate::store::archive_classmap_sidecar(&archive);
        assert_ne!(v0, v1, "v0 and v1 must be different files");
        assert!(
            SidecarV1::read(&v1).get(&key, &archive).is_none(),
            "a v0-only sidecar must miss under v1, not be read as a fallback"
        );

        let shaped = vec![(archive.join("Foo.php"), vec![b"Foo".to_vec()])];
        let mut cache = SidecarV1::default();
        cache
            .insert_and_write(&v1, &key, &archive, &shaped)
            .expect("v1 write should succeed");
        assert!(v1.is_file(), "a miss rewrites a fresh v1 sidecar");
        assert!(v0.is_file(), "the stale v0 file is left alone");
        assert_eq!(
            SidecarV1::read(&v1).get(&key, &archive),
            Some(shaped),
            "the freshly written v1 sidecar hits on the next read"
        );
    }
}
