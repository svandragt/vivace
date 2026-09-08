//! `vendor/bin` proxies: a port of Composer's `BinaryInstaller`. vivace
//! targets Linux only, so this covers `installUnixyProxyBinaries` and
//! `generateUnixyProxyCode` (the PHP and `sh` proxy shapes); Windows'
//! `.bat` writer (`installFullBinaries`) is not ported.
//!
//! Path shortening reuses `autoload::generator`'s port of Composer's
//! `Filesystem::findShortestPath[Code]`, the same helpers the autoloader
//! uses to write `__DIR__`-relative paths.

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::bytes::Regex;
use serde::Deserialize;

use crate::autoload::generator::{
    find_shortest_path, find_shortest_path_code, normalize_path, path_str,
};
use crate::lock::Package;

/// `config.bin-compat`. Composer's `full` also writes a `.bat` proxy for
/// cmd.exe; vivace only targets Linux, so `full` behaves like `auto` here
/// bar a log line. `symlink` is a deprecated Composer synonym for the proxy
/// today (Composer 2.2+ treats it the same as `auto`), but vivace honours it
/// literally, as asked: a relative symlink straight to the target, no proxy
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinCompat {
    #[default]
    Auto,
    Symlink,
    Full,
}

impl<'de> Deserialize<'de> for BinCompat {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "auto" => Ok(BinCompat::Auto),
            "symlink" => Ok(BinCompat::Symlink),
            "full" => Ok(BinCompat::Full),
            other => Err(serde::de::Error::custom(format!(
                "invalid config.bin-compat value: {other}"
            ))),
        }
    }
}

/// Write `vendor/bin` for every `(package, install_path)` pair with `bin`
/// entries (`install_path` already resolved, `target-dir` included), then
/// delete proxies left over from packages no longer installed. Returns
/// warnings for skipped bins (missing file, a directory, or a bin that
/// resolves outside the package), mirroring `installBinaries`'s
/// `writeError` calls; vivace regenerates every install, so an existing
/// proxy is always overwritten rather than skipped with a conflict warning.
pub fn generate(
    vendor_dir: &Path,
    bin_dir: &Path,
    bin_compat: BinCompat,
    packages: &[(&Package, PathBuf)],
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    if packages.iter().all(|(package, _)| package.bin.is_empty()) {
        if bin_dir.is_dir() {
            let bin_dir = fs_err::canonicalize(bin_dir)?;
            let vendor_dir_real =
                fs_err::canonicalize(vendor_dir).unwrap_or_else(|_| vendor_dir.to_path_buf());
            remove_stale(&bin_dir, &vendor_dir_real, &HashSet::new())?;
        }
        return Ok(warnings);
    }

    fs_err::create_dir_all(bin_dir)?;
    let bin_dir = fs_err::canonicalize(bin_dir)?;
    let vendor_dir_real =
        fs_err::canonicalize(vendor_dir).unwrap_or_else(|_| vendor_dir.to_path_buf());
    let mode = proxy_mode()?;

    let mut kept = HashSet::new();
    for (package, install_path) in packages {
        for bin in &package.bin {
            if let Some(name) = install_one(
                package,
                install_path,
                bin,
                &bin_dir,
                &vendor_dir_real,
                bin_compat,
                mode,
                &mut warnings,
            )? {
                kept.insert(name);
            }
        }
    }
    remove_stale(&bin_dir, &vendor_dir_real, &kept)?;
    Ok(warnings)
}

/// One `bin` entry: validate the target, then write the proxy or symlink.
/// Returns the proxy's file name on success, so the caller can tell it apart
/// from stale leftovers.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors installBinaries' loop body; a one-off context struct would just move the params"
)]
fn install_one(
    package: &Package,
    install_path: &Path,
    bin: &str,
    bin_dir: &Path,
    vendor_dir_real: &Path,
    bin_compat: BinCompat,
    mode: u32,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    let target = install_path.join(bin);
    if !target.exists() {
        warnings.push(format!(
            "Skipped installation of bin {bin} for package {}: file not found in package",
            package.name
        ));
        return Ok(None);
    }
    if target.is_dir() {
        warnings.push(format!(
            "Skipped installation of bin {bin} for package {}: found a directory at that path",
            package.name
        ));
        return Ok(None);
    }
    // GHSA-96h3-5x6v-m776: a malicious package can pass the metadata check
    // yet ship `bin` as a symlink pointing outside its own tree; follow it
    // and the proxy ends up serving an arbitrary host file.
    let canonical_target = fs_err::canonicalize(&target)?;
    let canonical_install = fs_err::canonicalize(install_path)?;
    if !canonical_target.starts_with(&canonical_install) {
        warnings.push(format!(
            "Skipped installation of bin {bin} for package {}: the bin resolves to a path \
             outside of the package directory",
            package.name
        ));
        return Ok(None);
    }
    // Composer also `chmod`s the target to add the execute bit;
    // vivace's store files are read-only hardlinks shared with every other
    // project on the machine, so the target keeps whatever mode the zip
    // gave it and only the proxy gets one here.

    let name = Path::new(bin)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(bin)
        .to_string();
    let link = bin_dir.join(&name);

    if bin_compat == BinCompat::Symlink {
        let relative = find_shortest_path(&path_str(&link), &path_str(&target), false);
        let _ = fs_err::remove_file(&link);
        std::os::unix::fs::symlink(relative, &link)?;
        return Ok(Some(name));
    }

    let content = unixy_proxy_code(&canonical_target, &link, vendor_dir_real)?;
    write_proxy(&link, content.as_bytes(), mode)?;
    Ok(Some(name))
}

/// `BinaryInstaller::generateUnixyProxyCode`: a PHP proxy when the target
/// starts with `<?php` (with an optional shebang), else an `sh` one that
/// `cd`s to the target's directory and `exec`s it.
fn unixy_proxy_code(target: &Path, link: &Path, vendor_dir_real: &Path) -> Result<String> {
    let target_s = path_str(target);
    let link_s = path_str(link);
    let bin_path = find_shortest_path(&link_s, &target_s, false);

    let mut head = [0u8; 500];
    let mut file = fs_err::File::open(target)?;
    let n = file.read(&mut head)?;

    let Some(detection) = detect_php(&head[..n]) else {
        let dir_part = dirname(&bin_path);
        let file_part = basename(&bin_path);
        return Ok(SH_PROXY_TEMPLATE
            .replace("__BIN_DIR__", &shell_escape(dir_part))
            .replace("__BIN_FILE__", file_part));
    };

    let shebang = detection
        .shebang
        .unwrap_or_else(|| "#!/usr/bin/env php".to_string());
    let bin_path_exported = find_shortest_path_code(&link_s, &target_s, false, true);
    let autoload_path = format!("{}/autoload.php", path_str(vendor_dir_real));
    let autoload_code = find_shortest_path_code(&link_s, &autoload_path, false, true);
    let mut globals = format!(
        "$GLOBALS['_composer_bin_dir'] = __DIR__;\n\
         $GLOBALS['_composer_autoload_path'] = {autoload_code};\n"
    );
    // PHPUnit process isolation workaround: keyed on the target's own path
    // matching `<vendor>/phpunit/phpunit/phpunit`, not the package name or
    // bin basename (`generateUnixyProxyCode`'s `$this->filesystem->normalizePath($bin)
    // === ... normalizePath($this->vendorDir.'/phpunit/phpunit/phpunit')`).
    let phpunit_hack = normalize_path(&target_s)
        == normalize_path(&format!(
            "{}/phpunit/phpunit/phpunit",
            path_str(vendor_dir_real)
        ));
    if phpunit_hack {
        globals.push_str("$GLOBALS['__PHPUNIT_ISOLATION_EXCLUDE_LIST'] = $GLOBALS['__PHPUNIT_ISOLATION_BLACKLIST'] = array(realpath(");
        globals.push_str(&bin_path_exported);
        globals.push_str("));\n");
    }
    let (stream_hint, stream_block) = if detection.needs_wrapper {
        (
            " using a stream wrapper to prevent the shebang from being output on PHP<8\n *"
                .to_string(),
            stream_wrapper_code(&bin_path_exported, phpunit_hack),
        )
    } else {
        (String::new(), String::new())
    };
    Ok(format!(
        "{shebang}\n<?php\n\n\
         /**\n\
         \x20* Proxy PHP file generated by Composer\n\
         \x20*\n\
         \x20* This file includes the referenced bin path ({bin_path})\n\
         \x20*{stream_hint}\n\
         \x20* @generated\n\
         \x20*/\n\n\
         namespace Composer;\n\n\
         {globals}\n\
         {stream_block}\n\
         return include {bin_path_exported};\n"
    ))
}

/// A target's first bytes matched Composer's
/// `{^(#!.*\r?\n)?[\r\n\t ]*<\?php}`: the optional shebang line it carried
/// over verbatim, and whether the match includes anything beyond a bare
/// `<?php` (a shebang or leading blank lines), which is when Composer wraps
/// the include in the `BinProxyWrapper` stream to hide the shebang from
/// PHP<8.
struct PhpDetection {
    shebang: Option<String>,
    needs_wrapper: bool,
}

fn detect_php(head: &[u8]) -> Option<PhpDetection> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(#!.*\r?\n)?[\r\n\t ]*<\?php").expect("valid regex"));
    let caps = RE.captures(head)?;
    let whole = caps.get(0).expect("group 0 always matches").as_bytes();
    let shebang = caps
        .get(1)
        .map(|m| String::from_utf8_lossy(m.as_bytes()).trim().to_string());
    let needs_wrapper = String::from_utf8_lossy(whole).trim() != "<?php";
    Some(PhpDetection {
        shebang,
        needs_wrapper,
    })
}

/// `ProcessExecutor::escapeArgument` on non-Windows: single-quote, doubling
/// any embedded quote.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// PHP's `dirname`/`basename` for the `/`-separated relative path
/// `find_shortest_path` returns (not a filesystem path, so `std::path::Path`
/// is not used here).
fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(pos) => &path[..pos],
        None => ".",
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `0777 & ~umask()`, read off a freshly made directory: `mkdir` always
/// requests `0777` before the kernel applies the umask, so the resulting
/// mode *is* that formula, no `libc` call needed.
fn proxy_mode() -> Result<u32> {
    let probe = tempfile::tempdir().context("probing the process umask")?;
    Ok(fs_err::metadata(probe.path())?.permissions().mode() & 0o777)
}

/// Write `content` to `path` (a proxy, so it needs `mode`) via a temp file
/// in the same directory renamed over the target, same as `install::write_atomic`.
fn write_proxy(path: &Path, content: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temp file next to {}", path.display()))?;
    temp.write_all(content)?;
    fs_err::set_permissions(temp.path(), PermissionsExt::from_mode(mode))?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// `BinaryInstaller::removeBinaries`, run for every package at once instead
/// of per-uninstall: delete anything in `bin_dir` that isn't one of this
/// install's proxies and is recognisably ours (a `@generated by Composer`
/// PHP proxy, an `sh` proxy carrying `SH_PROXY_TEMPLATE`'s own wording, or a
/// symlink into `vendor/`), never a file we don't recognise.
///
/// #29/#37: the `sh` proxy has no `@generated` marker (Composer's own
/// doesn't either — real Composer instead removes a package's own bins by
/// name from its own metadata, never scanning `bin_dir`), so recognising a
/// stale one here without inventing a marker that would change its bytes
/// (and fail the byte-golden test against Composer's output) means matching
/// text `SH_PROXY_TEMPLATE` already carries verbatim: a stray shell script a
/// package ships would essentially never contain this exact comment.
fn remove_stale(bin_dir: &Path, vendor_dir_real: &Path, keep: &HashSet<String>) -> Result<()> {
    for entry in fs_err::read_dir(bin_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if keep.contains(&name.to_string_lossy().into_owned()) {
            continue;
        }
        let path = entry.path();
        let is_symlink_into_vendor = entry.file_type()?.is_symlink()
            && fs_err::canonicalize(&path)
                .is_ok_and(|resolved| resolved.starts_with(vendor_dir_real));
        let looks_generated = !is_symlink_into_vendor
            && fs_err::read_to_string(&path).is_ok_and(|content| {
                content.contains("generated by Composer")
                    || content.contains("Support bash to support `source`")
            });
        if is_symlink_into_vendor || looks_generated {
            fs_err::remove_file(&path)?;
        }
    }
    Ok(())
}

/// `BinaryInstaller::generateUnixyProxyCode`'s PHP-file body when the target
/// starts with a shebang (or leading blank lines) before `<?php`: a
/// `stream_wrapper_register`-backed `BinProxyWrapper` that strips the
/// shebang line so PHP<8 doesn't echo it back out. Verbatim from Composer,
/// bar two splice points: the target's own path between head and tail, and
/// (when `phpunit_hack` is set) the `PHPUnit` process-isolation lines PHP
/// itself only adds inside the `$phpunitHack1`/`$phpunitHack2` branch.
fn stream_wrapper_code(bin_path_exported: &str, phpunit_hack: bool) -> String {
    let opened_path_value = if phpunit_hack {
        "'phpvfscomposer://'.$this->realpath"
    } else {
        "$this->realpath"
    };
    let read_hack = if phpunit_hack {
        "\n                $data = str_replace('__DIR__', var_export(dirname($this->realpath), true), $data);\
         \n                $data = str_replace('__FILE__', var_export($this->realpath, true), $data);"
    } else {
        ""
    };
    format!(
        "{PHP_STREAM_WRAPPER_HEAD}{opened_path_value};{PHP_STREAM_WRAPPER_MID}{read_hack}{PHP_STREAM_WRAPPER_TAIL_A}{bin_path_exported}{PHP_STREAM_WRAPPER_TAIL_B}"
    )
}

const PHP_STREAM_WRAPPER_HEAD: &str = r"if (PHP_VERSION_ID < 80000) {
    if (!class_exists('Composer\BinProxyWrapper')) {
        /**
         * @internal
         */
        final class BinProxyWrapper
        {
            private $handle;
            private $position;
            private $realpath;

            public function stream_open($path, $mode, $options, &$opened_path)
            {
                // get rid of phpvfscomposer:// prefix for __FILE__ & __DIR__ resolution
                $opened_path = substr($path, 17);
                $this->realpath = realpath($opened_path) ?: $opened_path;
                $opened_path = ";

const PHP_STREAM_WRAPPER_MID: &str = r"
                $this->handle = fopen($this->realpath, $mode);
                $this->position = 0;

                return (bool) $this->handle;
            }

            public function stream_read($count)
            {
                $data = fread($this->handle, $count);

                if ($this->position === 0) {
                    $data = preg_replace('{^#!.*\r?\n}', '', $data);
                }";

const PHP_STREAM_WRAPPER_TAIL_A: &str = r#"

                $this->position += strlen($data);

                return $data;
            }

            public function stream_cast($castAs)
            {
                return $this->handle;
            }

            public function stream_close()
            {
                fclose($this->handle);
            }

            public function stream_lock($operation)
            {
                return $operation ? flock($this->handle, $operation) : true;
            }

            public function stream_seek($offset, $whence)
            {
                if (0 === fseek($this->handle, $offset, $whence)) {
                    $this->position = ftell($this->handle);
                    return true;
                }

                return false;
            }

            public function stream_tell()
            {
                return $this->position;
            }

            public function stream_eof()
            {
                return feof($this->handle);
            }

            public function stream_stat()
            {
                return array();
            }

            public function stream_set_option($option, $arg1, $arg2)
            {
                return true;
            }

            public function url_stat($path, $flags)
            {
                $path = substr($path, 17);
                if (file_exists($path)) {
                    return stat($path);
                }

                return false;
            }
        }
    }

    if (
        (function_exists('stream_get_wrappers') && in_array('phpvfscomposer', stream_get_wrappers(), true))
        || (function_exists('stream_wrapper_register') && stream_wrapper_register('phpvfscomposer', 'Composer\BinProxyWrapper'))
    ) {
        return include("phpvfscomposer://" . "#;

const PHP_STREAM_WRAPPER_TAIL_B: &str = ");\n    }\n}\n";

/// `BinaryInstaller::generateUnixyProxyCode`'s `sh`-file body, verbatim,
/// with `__BIN_DIR__`/`__BIN_FILE__` standing in for `$binDir`/`$binFile`
/// (a `format!` literal can't hold this much of the script's own `{}`).
const SH_PROXY_TEMPLATE: &str = r#"#!/usr/bin/env sh

# Support bash to support `source` with fallback on $0 if this does not run with bash
# https://stackoverflow.com/a/35006505/6512
selfArg="$BASH_SOURCE"
if [ -z "$selfArg" ]; then
    selfArg="$0"
fi

self=$(realpath "$selfArg" 2> /dev/null)
if [ -z "$self" ]; then
    self="$selfArg"
fi

dir=$(cd "${self%[/\\]*}" > /dev/null; cd __BIN_DIR__ && pwd)

if [ -d /proc/cygdrive ]; then
    case $(which php) in
        $(readlink -n /proc/cygdrive)/*)
            # We are in Cygwin using Windows php, so the path must be translated
            dir=$(cygpath -m "$dir");
            ;;
    esac
fi

export COMPOSER_RUNTIME_BIN_DIR="$(cd "${self%[/\\]*}" > /dev/null; pwd)"

# If bash is sourcing this file, we have to source the target as well
bashSource="$BASH_SOURCE"
if [ -n "$bashSource" ]; then
    if [ "$bashSource" != "$0" ]; then
        source "${dir}/__BIN_FILE__" "$@"
        return
    fi
fi

exec "${dir}/__BIN_FILE__" "$@"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shebang_php() {
        let d = detect_php(b"#!/usr/bin/env php\n<?php\necho 1;").unwrap();
        assert_eq!(d.shebang.as_deref(), Some("#!/usr/bin/env php"));
        assert!(d.needs_wrapper);
    }

    /// The branch none of the seven goldens exercise: a target that opens
    /// with a bare `<?php`, no shebang, gets the default shebang and skips
    /// the `BinProxyWrapper` entirely (`trim($match[0]) === '<?php'`).
    #[test]
    fn plain_php_target_needs_no_wrapper() {
        let d = detect_php(b"<?php\necho 1;").unwrap();
        assert_eq!(d.shebang, None);
        assert!(!d.needs_wrapper);
    }

    #[test]
    fn non_php_target_is_not_detected() {
        assert!(detect_php(b"#!/usr/bin/env sh\necho hi\n").is_none());
    }

    #[test]
    fn shell_escape_doubles_embedded_quotes() {
        assert_eq!(
            shell_escape("../fig-r/psr2r-sniffer/bin"),
            "'../fig-r/psr2r-sniffer/bin'"
        );
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn plain_php_proxy_has_no_stream_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        let install_path = vendor.join("acme/tool");
        fs_err::create_dir_all(&install_path).unwrap();
        fs_err::write(install_path.join("run"), "<?php\necho 1;\n").unwrap();
        fs_err::create_dir_all(vendor.join("bin")).unwrap();

        let content = unixy_proxy_code(
            &install_path.join("run"),
            &vendor.join("bin/run"),
            &fs_err::canonicalize(&vendor).unwrap(),
        )
        .unwrap();

        assert!(content.starts_with("#!/usr/bin/env php\n<?php\n"));
        assert!(!content.contains("BinProxyWrapper"));
        assert!(content.ends_with("return include __DIR__ . '/..'.'/acme/tool/run';\n"));
    }
}
