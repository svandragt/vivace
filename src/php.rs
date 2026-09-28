//! `viv php install [VERSION]`/`viv php list` (#337): download a
//! static-php-cli `cli` build for the running OS/arch into the store and
//! pin `config.platform.php` to it, so a project's PHP no longer has to
//! come from whatever happens to be on `PATH`.
//!
//! Composer has no such command; this is entirely vivace's own, kept out of
//! `install.rs`/`solver::platform` (which only ever *detect* an existing
//! `php`, never fetch one) rather than folded into either.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use regex::Regex;
use reqwest::Url;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::auth::Auth;
use crate::fetch::{Conditional, Downloaded, Fetcher};
use crate::normalize;
use crate::require::write_composer_json;
use crate::store::{self, PHP_BUCKET};

/// `viv php` flags: which PHP operation to run.
#[derive(Args, Debug, Clone)]
pub struct PhpArgs {
    #[command(subcommand)]
    pub command: PhpCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum PhpCommand {
    /// Download a static-php-cli build and pin `config.platform.php` in
    /// composer.json to it. With no `VERSION`, installs whatever is
    /// already pinned there.
    Install {
        /// `8.4` (newest `8.4.x` upstream) or `8.4.17` (exact).
        version: Option<String>,
    },
    /// List every PHP build already installed in the cache.
    List,
}

pub fn run(args: &PhpArgs, cache_dir: Option<&Path>) -> Result<()> {
    match &args.command {
        PhpCommand::Install { version } => run_install(version.as_deref(), cache_dir),
        PhpCommand::List => run_list(cache_dir),
    }
}

fn resolve_cache_dir(cache_dir: Option<&Path>) -> Result<PathBuf> {
    match cache_dir {
        Some(dir) => Ok(dir.to_path_buf()),
        None => crate::update::default_cache_dir(),
    }
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout`
/// lint (`tool.rs`/`workspace.rs`'s own `out` do the same).
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint (`require.rs`'s own `warn_out` does the same).
fn warn_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// A `VERSION` argument's shape: `8.4` resolves to the newest `8.4.x`
/// upstream release; `8.4.17` names an exact one. Anything else (`8`,
/// `8.4.x`, `latest`) is rejected before it ever reaches the listing regex.
/// A `config.platform.php` pin read back (always `X.Y.Z`) parses the same
/// way and lands on `Exact`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionSpec {
    Minor(String),
    Exact(String),
}

fn parse_version(input: &str) -> Result<VersionSpec> {
    let parts: Vec<&str> = input.split('.').collect();
    let all_digits = parts
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    match (parts.as_slice(), all_digits) {
        ([_, _], true) => Ok(VersionSpec::Minor(input.to_string())),
        ([_, _, _], true) => Ok(VersionSpec::Exact(input.to_string())),
        _ => {
            bail!("invalid PHP version {input:?}: use \"8.4\" (newest patch) or \"8.4.17\" (exact)")
        }
    }
}

const UNSUPPORTED_PLATFORM: &str = "viv php install supports linux and macOS on x86_64 and aarch64";

/// static-php-cli's own OS/arch names, which happen to be spelled exactly
/// like Rust's `std::env::consts` for every combination it publishes; this
/// is still a named mapping (not a bare pass-through) so an unsupported
/// pair is rejected here, once, rather than reaching a 404 at download
/// time.
fn map_platform(os: &str, arch: &str) -> Result<(&'static str, &'static str)> {
    let os = match os {
        "linux" => "linux",
        "macos" => "macos",
        _ => bail!(UNSUPPORTED_PLATFORM),
    };
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => bail!(UNSUPPORTED_PLATFORM),
    };
    Ok((os, arch))
}

fn platform() -> Result<(&'static str, &'static str)> {
    map_platform(std::env::consts::OS, std::env::consts::ARCH)
}

const DEFAULT_DIST_BASE: &str = "https://dl.static-php.dev/static-php-cli/bulk";

/// The `bulk` bundle's base URL (it includes `intl`; `common` does not).
/// Overridable only by `VIV_PHP_DIST_URL`, and only so tests can point it
/// at a local server instead of the real one.
fn dist_base_url() -> String {
    std::env::var("VIV_PHP_DIST_URL").unwrap_or_else(|_| DEFAULT_DIST_BASE.to_string())
}

/// The newest `<minor>.<patch>` the `bulk/` directory listing names for
/// `os`/`arch`, or `None` if it names none. Pure so a fixture string covers
/// it without a network round trip.
fn newest_patch(listing: &str, minor: &str, os: &str, arch: &str) -> Option<String> {
    let pattern = format!(
        r"php-{}\.(\d+)-cli-{}-{}\.tar\.gz",
        regex::escape(minor),
        regex::escape(os),
        regex::escape(arch)
    );
    let re = Regex::new(&pattern).ok()?;
    let max_patch = re
        .captures_iter(listing)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max()?;
    Some(format!("{minor}.{max_patch}"))
}

/// `<cache_dir>/php-v0/<version>-<os>-<arch>/`.
fn install_dir(cache_dir: &Path, version: &str, os: &str, arch: &str) -> PathBuf {
    cache_dir
        .join(PHP_BUCKET)
        .join(format!("{version}-{os}-{arch}"))
}

/// Written last, once the binary and `.sha256` are both in place —
/// `store.rs`'s own `archive-v0` convention for "extraction finished", see
/// its module doc comment.
const OK_MARKER: &str = ".ok";
const SHA256_MARKER: &str = ".sha256";

fn run_install(version_arg: Option<&str>, cache_dir: Option<&Path>) -> Result<()> {
    let cache_dir = resolve_cache_dir(cache_dir)?;
    let project_dir = std::env::current_dir().context("resolving the current directory")?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json_exists = composer_json_path.is_file();

    let version_input = match version_arg {
        Some(v) => v.to_string(),
        None => pinned_version(&composer_json_path, composer_json_exists)?,
    };
    let spec = parse_version(&version_input)?;
    let (os, arch) = platform()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // Plain http only for the test server named by `VIV_PHP_DIST_URL`;
    // upstream publishes no checksums, so an http redirect in production
    // would be an unverified binary.
    let fetcher =
        Fetcher::new(Auth::default())?.secure_http(std::env::var_os("VIV_PHP_DIST_URL").is_none());

    let resolved_version = match spec {
        VersionSpec::Exact(v) => v,
        VersionSpec::Minor(minor) => runtime.block_on(resolve_minor(&fetcher, &minor, os, arch))?,
    };

    let dest = install_dir(&cache_dir, &resolved_version, os, arch);
    let php_path = dest.join("php");
    if dest.join(OK_MARKER).is_file() {
        out(&format!("php {resolved_version} already installed"));
    } else {
        runtime.block_on(download_and_install(
            &fetcher,
            &cache_dir,
            &dest,
            &resolved_version,
            os,
            arch,
        ))?;
        out(&format!(
            "installed php {resolved_version} ({os}-{arch}) to {}",
            php_path.display()
        ));
    }

    if version_arg.is_some() {
        if composer_json_exists {
            write_platform_pin(&composer_json_path, &version_input)?;
        } else {
            warn_out("composer.json not found; installed php only");
        }
    }
    Ok(())
}

fn pinned_version(composer_json_path: &Path, exists: bool) -> Result<String> {
    if exists && let Some(pin) = read_platform_php_pin(composer_json_path)? {
        return Ok(pin);
    }
    bail!(
        "no PHP pinned: set config.platform.php in composer.json or run viv php install <version>"
    );
}

/// `config.platform.php` from a composer.json, or `None` when the file is
/// absent or names no pin — shared by `pinned_version` (`viv php install`'s
/// own "no pin" is an error) and [`project_php_dir`] (`viv run`/`viv exec`'s
/// own "no pin" is just "use whatever's on PATH").
fn read_platform_php_pin(composer_json_path: &Path) -> Result<Option<String>> {
    if !composer_json_path.is_file() {
        return Ok(None);
    }
    let root = crate::lock::read_root(composer_json_path)?;
    Ok(root
        .config
        .platform
        .get("php")
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// The directory holding the project's pinned PHP's `php` binary, or `None`
/// when `composer.json` is absent or pins nothing — the "just use PATH"
/// case `viv run`/`viv exec` fall back to. An exact pin (`8.4.17`) or a
/// minor one already installed (`8.4` -> its newest installed `8.4.x`) that
/// isn't actually installed is an error, not `None`: a project that *does*
/// pin a PHP and doesn't have it installed should say so, not silently run
/// whatever's on the caller's PATH instead.
pub fn project_php_dir(project_dir: &Path, cache_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let composer_json_path = project_dir.join("composer.json");
    let Some(pin) = read_platform_php_pin(&composer_json_path)? else {
        return Ok(None);
    };
    let cache_dir = resolve_cache_dir(cache_dir)?;
    let (os, arch) = platform()?;
    let version = match parse_version(&pin)? {
        VersionSpec::Exact(v) => v,
        VersionSpec::Minor(minor) => newest_installed_patch(&cache_dir, &minor, os, arch)?
            .ok_or_else(|| anyhow::anyhow!(not_installed_message(&pin)))?,
    };
    let dest = install_dir(&cache_dir, &version, os, arch);
    if !dest.join(OK_MARKER).is_file() {
        bail!(not_installed_message(&pin));
    }
    Ok(Some(dest))
}

fn not_installed_message(pin: &str) -> String {
    format!("php {pin} is pinned in composer.json but not installed; run viv php install")
}

/// The newest installed `<minor>.x` for `os`/`arch`, from the same cache
/// scan [`run_list`] does (no network) — the minor-pin half of
/// [`project_php_dir`]'s lookup.
fn newest_installed_patch(
    cache_dir: &Path,
    minor: &str,
    os: &str,
    arch: &str,
) -> Result<Option<String>> {
    let platform = format!("{os}-{arch}");
    let minor_key = version_key(minor);
    Ok(installed_entries(cache_dir)?
        .into_iter()
        .filter(|(version, entry_platform)| {
            *entry_platform == platform && version_key(version).starts_with(&minor_key)
        })
        .map(|(version, _)| version)
        .max_by_key(|version| version_key(version)))
}

/// `<php_dir>:<bin_dir>:<inherited PATH>`, each prefix included only when it
/// applies (`php_dir` is `Some`; `bin_dir` is an existing directory), and
/// never re-adding a prefix that already sits first in the inherited PATH —
/// `scripts.rs`'s own `apply_env` used to do this dedupe just for `bin_dir`;
/// this is that same check, shared, now covering `php_dir` too, for
/// `scripts::Runner::apply_env`, `tool::run_exec` and `tool::run_run`'s
/// vendor/bin and PATH fallbacks (#338).
pub fn compose_path(php_dir: Option<&Path>, bin_dir: &Path) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let first = path.split(':').next();
    let mut prefixes = Vec::new();
    if let Some(dir) = php_dir {
        prefixes.push(dir.display().to_string());
    }
    if bin_dir.is_dir() {
        prefixes.push(bin_dir.display().to_string());
    }
    prefixes.retain(|prefix| Some(prefix.as_str()) != first);
    if prefixes.is_empty() {
        return path;
    }
    prefixes.push(path);
    prefixes.join(":")
}

async fn resolve_minor(fetcher: &Fetcher, minor: &str, os: &str, arch: &str) -> Result<String> {
    let base = dist_base_url();
    let listing_url_str = format!("{base}/");
    let listing_url =
        Url::parse(&listing_url_str).with_context(|| format!("{listing_url_str}: invalid URL"))?;
    let body = match fetcher
        .get_conditional("static-php-cli listing", &listing_url, None)
        .await?
    {
        Conditional::Fresh { body, .. } => body,
        Conditional::NotModified => bail!("{listing_url_str}: unexpected 304 with no cached copy"),
        Conditional::NotFound => bail!("{listing_url_str}: not found"),
    };
    let listing = String::from_utf8(body).context("static-php-cli listing is not valid UTF-8")?;
    newest_patch(&listing, minor, os, arch)
        .with_context(|| format!("no php {minor}.x build found for {os}-{arch}"))
}

async fn download_and_install(
    fetcher: &Fetcher,
    cache_dir: &Path,
    dest: &Path,
    version: &str,
    os: &str,
    arch: &str,
) -> Result<()> {
    let bucket_dir = cache_dir.join(PHP_BUCKET);
    fs_err::create_dir_all(&bucket_dir)?;

    let base = dist_base_url();
    let url_str = format!("{base}/php-{version}-cli-{os}-{arch}.tar.gz");
    let url = Url::parse(&url_str).with_context(|| format!("{url_str}: invalid URL"))?;
    let downloaded = fetcher
        .get_raw(&format!("php {version} ({os}-{arch})"), url, &bucket_dir)
        .await
        .with_context(|| format!("downloading php {version} ({os}-{arch})"))?;

    let sha256_hex = hash_downloaded(&downloaded)?;

    // ponytail: a stale, incomplete `dest` (crashed mid-extraction) is
    // simply wiped and redone rather than swapped aside like `store.rs`'s
    // `store_extracted` does for a concurrent writer; two `viv php install`
    // runs racing the same version is rare enough to leave unhandled until
    // it's reported.
    let temp = tempfile::tempdir_in(&bucket_dir)?;
    extract_php_binary(&downloaded, temp.path())?;
    // Upstream static-php-cli publishes no checksums for these builds: this
    // hash is recorded for a future integrity check against a later
    // download of the same version, not to verify this one.
    fs_err::write(temp.path().join(SHA256_MARKER), &sha256_hex)?;
    let temp = temp.keep();

    if dest.is_dir() {
        fs_err::remove_dir_all(dest)?;
    }
    fs_err::rename(&temp, dest)?;
    fs_err::write(dest.join(OK_MARKER), b"")?;
    Ok(())
}

fn hash_downloaded(downloaded: &Downloaded) -> Result<String> {
    let mut hasher = Sha256::new();
    match downloaded {
        Downloaded::Bytes(bytes) => hasher.update(bytes),
        Downloaded::File(path) => {
            let mut file = fs_err::File::open(path)?;
            let mut buf = [0u8; 8 * 1024];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
    }
    Ok(store::hex(hasher.finalize()))
}

/// The `bulk` tarball is exactly one file, `php`, at its root (no
/// containing directory) — write it straight into `dest_dir` and mark it
/// executable.
fn extract_php_binary(downloaded: &Downloaded, dest_dir: &Path) -> Result<()> {
    match downloaded {
        Downloaded::Bytes(bytes) => {
            extract_php_from_tar_gz(std::io::Cursor::new(bytes.as_slice()), dest_dir)
        }
        Downloaded::File(path) => extract_php_from_tar_gz(fs_err::File::open(path)?, dest_dir),
    }
}

fn extract_php_from_tar_gz(reader: impl std::io::Read, dest_dir: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_php = entry
            .path()?
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "php");
        if !is_php {
            continue;
        }
        let dest = dest_dir.join("php");
        let mut out = fs_err::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        fs_err::set_permissions(&dest, PermissionsExt::from_mode(0o755))?;
        return Ok(());
    }
    bail!("static-php-cli tarball has no \"php\" entry");
}

/// `config.platform.php`'s own composer.json edit: the same parse-map-
/// serialize-then-normalize seam `require.rs`'s `add_link`/
/// `write_composer_json` use for `require`/`require-dev`, one level deeper
/// (`config.platform` rather than the root object).
fn write_platform_pin(path: &Path, version: &str) -> Result<()> {
    let original = fs_err::read_to_string(path).context("reading composer.json")?;
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    let obj = root
        .as_object_mut()
        .context("composer.json must be a JSON object")?;
    let config = obj
        .entry("config")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("config must be a JSON object")?;
    let platform = config
        .entry("platform")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("config.platform must be a JSON object")?;
    platform.insert("php".to_string(), Value::String(version.to_string()));

    let indent = normalize::detect_indent(&original);
    write_composer_json(path, &root)?;
    normalize::maybe_normalize(path, &indent)?;
    Ok(())
}

/// `<version>-<os>-<arch>` -> `(version, "<os>-<arch>")`, the reverse of
/// [`install_dir`]'s own file name; `os`/`arch` are always one of these
/// four pairs (see [`map_platform`]), so a plain suffix match is enough —
/// no need to parse the version out from the front instead.
const PLATFORM_SUFFIXES: [&str; 4] = [
    "-linux-x86_64",
    "-linux-aarch64",
    "-macos-x86_64",
    "-macos-aarch64",
];

fn split_dir_name(name: &str) -> Option<(String, String)> {
    for suffix in PLATFORM_SUFFIXES {
        if let Some(version) = name.strip_suffix(suffix) {
            let platform = suffix.trim_start_matches('-').to_string();
            return Some((version.to_string(), platform));
        }
    }
    None
}

/// `X.Y.Z` -> `[X, Y, Z]` for a numeric sort; a component that fails to
/// parse (never expected here, every entry comes from [`install_dir`])
/// sorts as `0` rather than panicking.
fn version_key(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect()
}

/// Every `(version, platform)` installed in the cache (an `.ok` marker
/// present), for [`run_list`] and [`newest_installed_patch`] to filter/sort
/// however each needs.
fn installed_entries(cache_dir: &Path) -> Result<Vec<(String, String)>> {
    let bucket_dir = cache_dir.join(PHP_BUCKET);
    let mut entries: Vec<(String, String)> = Vec::new();
    if bucket_dir.is_dir() {
        for entry in fs_err::read_dir(&bucket_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if !entry.path().join(OK_MARKER).is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(parsed) = split_dir_name(&name) {
                entries.push(parsed);
            }
        }
    }
    Ok(entries)
}

fn run_list(cache_dir: Option<&Path>) -> Result<()> {
    let cache_dir = resolve_cache_dir(cache_dir)?;
    let mut entries = installed_entries(&cache_dir)?;
    entries.sort_by_key(|(version, _)| version_key(version));
    entries.reverse();
    for (version, platform) in entries {
        out(&format!("{version} {platform}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        VersionSpec, compose_path, map_platform, newest_installed_patch, newest_patch,
        parse_version, split_dir_name, version_key,
    };

    #[test]
    fn parse_version_accepts_minor() {
        assert_eq!(
            parse_version("8.4").unwrap(),
            VersionSpec::Minor("8.4".to_string())
        );
    }

    #[test]
    fn parse_version_accepts_exact() {
        assert_eq!(
            parse_version("8.4.17").unwrap(),
            VersionSpec::Exact("8.4.17".to_string())
        );
    }

    #[test]
    fn parse_version_rejects_bare_major() {
        assert!(parse_version("8").is_err());
    }

    #[test]
    fn parse_version_rejects_a_wildcard_patch() {
        assert!(parse_version("8.4.x").is_err());
    }

    #[test]
    fn parse_version_rejects_latest() {
        assert!(parse_version("latest").is_err());
    }

    const LISTING_FIXTURE: &str = r#"
        <a href="php-8.3.15-cli-linux-x86_64.tar.gz">php-8.3.15-cli-linux-x86_64.tar.gz</a>
        <a href="php-8.4.5-cli-linux-x86_64.tar.gz">php-8.4.5-cli-linux-x86_64.tar.gz</a>
        <a href="php-8.4.17-cli-linux-x86_64.tar.gz">php-8.4.17-cli-linux-x86_64.tar.gz</a>
        <a href="php-8.4.9-cli-linux-x86_64.tar.gz">php-8.4.9-cli-linux-x86_64.tar.gz</a>
        <a href="php-8.4.23-cli-macos-aarch64.tar.gz">php-8.4.23-cli-macos-aarch64.tar.gz</a>
    "#;

    #[test]
    fn newest_patch_picks_the_max_for_the_requested_minor_and_platform() {
        assert_eq!(
            newest_patch(LISTING_FIXTURE, "8.4", "linux", "x86_64").as_deref(),
            Some("8.4.17")
        );
    }

    #[test]
    fn newest_patch_is_platform_specific() {
        assert_eq!(
            newest_patch(LISTING_FIXTURE, "8.4", "macos", "aarch64").as_deref(),
            Some("8.4.23")
        );
    }

    #[test]
    fn newest_patch_none_for_an_absent_minor() {
        assert_eq!(
            newest_patch(LISTING_FIXTURE, "8.5", "linux", "x86_64"),
            None
        );
    }

    #[test]
    fn map_platform_accepts_the_two_supported_combinations() {
        assert_eq!(
            map_platform("linux", "x86_64").unwrap(),
            ("linux", "x86_64")
        );
        assert_eq!(
            map_platform("macos", "aarch64").unwrap(),
            ("macos", "aarch64")
        );
    }

    #[test]
    fn map_platform_rejects_windows() {
        assert!(map_platform("windows", "x86_64").is_err());
    }

    #[test]
    fn split_dir_name_round_trips_install_dir() {
        assert_eq!(
            split_dir_name("8.4.17-linux-x86_64"),
            Some(("8.4.17".to_string(), "linux-x86_64".to_string()))
        );
    }

    #[test]
    fn version_key_sorts_numerically_not_lexically() {
        assert!(version_key("8.4.9") < version_key("8.4.17"));
    }

    #[test]
    fn compose_path_prepends_php_dir_then_bin_dir() {
        let bin_dir = std::env::current_dir().unwrap(); // any existing dir
        let composed = compose_path(Some(Path::new("/opt/php")), &bin_dir);
        assert_eq!(
            composed,
            format!(
                "/opt/php:{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            )
        );
    }

    #[test]
    fn compose_path_skips_a_bin_dir_already_first_in_path() {
        let path = std::env::var("PATH").unwrap_or_default();
        let first = path.split(':').next().unwrap();
        let composed = compose_path(None, Path::new(first));
        assert_eq!(composed, path);
    }

    #[test]
    fn compose_path_skips_a_missing_bin_dir() {
        let composed = compose_path(None, Path::new("/does/not/exist"));
        assert_eq!(composed, std::env::var("PATH").unwrap_or_default());
    }

    /// `<cache>/php-v0/<version>-<os>-<arch>/.ok`, mirroring [`super::install_dir`]
    /// / [`super::OK_MARKER`] without depending on a real download.
    fn fake_install(cache_dir: &Path, version: &str, os: &str, arch: &str) {
        let dir = cache_dir
            .join(super::PHP_BUCKET)
            .join(format!("{version}-{os}-{arch}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(super::OK_MARKER), b"").unwrap();
    }

    #[test]
    fn newest_installed_patch_picks_the_max_for_the_minor_and_platform() {
        let cache = tempfile::tempdir().unwrap();
        fake_install(cache.path(), "8.4.5", "linux", "x86_64");
        fake_install(cache.path(), "8.4.17", "linux", "x86_64");
        fake_install(cache.path(), "8.4.9", "linux", "x86_64");
        fake_install(cache.path(), "8.4.99", "macos", "aarch64");

        assert_eq!(
            newest_installed_patch(cache.path(), "8.4", "linux", "x86_64").unwrap(),
            Some("8.4.17".to_string())
        );
    }

    #[test]
    fn newest_installed_patch_none_when_nothing_matches() {
        let cache = tempfile::tempdir().unwrap();
        fake_install(cache.path(), "8.3.5", "linux", "x86_64");

        assert_eq!(
            newest_installed_patch(cache.path(), "8.4", "linux", "x86_64").unwrap(),
            None
        );
    }
}
