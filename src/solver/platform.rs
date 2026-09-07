//! Port of `Repository/PlatformRepository.php`'s fixed platform packages
//! (`initialize`, `addExtension`, `addLibrary`, `buildPackageName`) and
//! `Platform/Version.php`'s per-library version parsing. Split out of
//! `pool_builder.rs` because the extension/library switch alone is well
//! over 300 lines.
//!
//! One `php` subprocess (`probe`) does everything only PHP can do: list
//! loaded extensions and scrape `ReflectionExtension::info()` (phpinfo's
//! per-extension dump) with the same regexes Composer's `switch ($name)`
//! block uses, so Rust never has to parse that free-text format. Numeric
//! version *normalisation* — the openssl letter-suffix table, PCRE/libjpeg/
//! libxpm/openldap encodings — stays in Rust (`parse_openssl`,
//! `parse_libjpeg`, `convert_version_id`), mirroring `Platform/Version.php`
//! line for line so a fixture's expected version is easy to check against
//! that file rather than against this scrape.
//!
//! `overrides` is `config.platform` verbatim: a name -> pretty-version
//! string pins that platform package's version instead of detecting it
//! from the host, and a name -> `false` removes it outright
//! (`PlatformRepository`'s own `platform-overrides`/`platform` handling).
//! Hermetic tests use this to avoid depending on the host `php` build. A
//! name not already detected on the host is still added verbatim if
//! overridden (matching this function's pre-existing behaviour, not
//! `PlatformRepository::addOverriddenPackage`'s own "no-op if nothing to
//! override" case — narrowing that is left for when a fixture needs it).

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::Result;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::semver;
use crate::solver::pool::{Link, Package};

/// One `platform_packages()` entry before overrides/normalisation:
/// `PlatformRepository::addExtension`/`addLibrary`'s `CompletePackage` plus
/// its `replaces`/`provides` links, both by bare target name (`lib-`/`ext-`
/// prefix added by the caller, matching `addLibrary`'s own
/// `'lib-'.$replace`/`'lib-'.$provide`).
struct Entry {
    name: String,
    pretty_version: String,
    provides: Vec<String>,
    replaces: Vec<String>,
}

impl Entry {
    fn new(name: impl Into<String>, pretty_version: impl Into<String>) -> Self {
        Entry {
            name: name.into(),
            pretty_version: pretty_version.into(),
            provides: Vec::new(),
            replaces: Vec::new(),
        }
    }
}

/// The raw JSON the probe script prints: everything `PlatformRepository::
/// initialize` reads from `Runtime`/`phpinfo()`, already reduced to the
/// clean strings its own regexes extract (`grab()` in the probe script is
/// the same pattern per case). `#[serde(default)]` so a probe from an older
/// `viv` build (or a hand-written fixture) missing a newer field just
/// leaves that library undetected instead of failing to parse.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the probe script's own flat JSON fields"
)]
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Probe {
    php_version: String,
    int_size: i64,
    zts: bool,
    debug: bool,
    ipv6: bool,
    hhvm_version: Option<String>,
    extensions: Map<String, Value>,

    amqp_librabbitmq: Option<String>,
    amqp_protocol: Option<String>,
    bz2_version: Option<String>,
    curl_version: Option<String>,
    curl_ssl_library: Option<String>,
    curl_ssl_version: Option<String>,
    curl_ssh_library: Option<String>,
    curl_ssh_version: Option<String>,
    curl_zlib_version: Option<String>,
    date_timelib: Option<String>,
    date_zoneinfo_external: Option<bool>,
    date_zoneinfo_version: Option<String>,
    timezonedb_loaded: bool,
    fileinfo_libmagic: Option<String>,
    gd_version: Option<String>,
    gd_libjpeg: Option<String>,
    gd_libpng: Option<String>,
    gd_freetype: Option<String>,
    gd_libxpm_id: Option<i64>,
    gmp_version: Option<String>,
    iconv_version: Option<String>,
    icu_version: Option<String>,
    icu_tzdata: Option<String>,
    icu_cldr: Option<String>,
    icu_unicode: Option<String>,
    imagick_version_string: Option<String>,
    ldap_vendor_version_id: Option<i64>,
    ldap_vendor_name: Option<String>,
    libxml_version: Option<String>,
    libxml_provides: Vec<String>,
    mbstring_libmbfl: Option<String>,
    mbstring_oniguruma: Option<String>,
    memcached_libmemcached: Option<String>,
    mongodb_libmongoc: Option<String>,
    mongodb_libbson: Option<String>,
    openssl_version_text: Option<String>,
    pcre_version: Option<String>,
    pcre_unicode: Option<String>,
    mysqlnd_mysqlnd: Option<String>,
    pdo_mysql_mysqlnd: Option<String>,
    pgsql_libpq: Option<String>,
    pgsql_libpq_info: Option<String>,
    pdo_pgsql_libpq: Option<String>,
    pq_libpq_linked: Option<String>,
    rdkafka_version_int: Option<i64>,
    sodium_library_version: Option<String>,
    sqlite3_sqlite: Option<String>,
    pdo_sqlite_sqlite: Option<String>,
    ssh2_libssh2: Option<String>,
    xsl_libxslt: Option<String>,
    xsl_libxslt_libxml: Option<String>,
    yaml_libyaml: Option<String>,
    zip_libzip: Option<String>,
    zlib_version: Option<String>,
}

/// The PHP side of the port: `PlatformRepository::initialize`'s extension
/// switch, minus the version-normalisation math (that part stays in Rust,
/// see the module doc comment). Every regex here is copied verbatim from
/// `PlatformRepository.php` (`Preg::isMatch`/`isMatchStrictGroups` behave
/// like plain `preg_match` for this module's purposes: neither call site
/// relies on the strict-groups "every named group must match" distinction).
/// Piped to plain `php`'s stdin (no `-r`/`-f` argument, see `run_probe`) so
/// none of these regexes need shell-quoting.
const PROBE_SCRIPT: &str = r#"<?php
function ext_info(string $ext): string {
    if (!extension_loaded($ext)) {
        return '';
    }
    try {
        $r = new ReflectionExtension($ext);
        ob_start();
        $r->info();
        return (string) ob_get_clean();
    } catch (\Throwable $e) {
        return '';
    }
}

function grab(string $pattern, string $subject): ?string {
    return preg_match($pattern, $subject, $m) === 1 ? $m['version'] : null;
}

$loaded = get_loaded_extensions();
$exts = [];
foreach ($loaded as $name) {
    if (in_array($name, ['standard', 'Core'], true)) {
        continue;
    }
    $v = phpversion($name);
    $exts[$name] = $v === false ? '0' : $v;
}

$out = [
    'php_version' => PHP_VERSION,
    'int_size' => PHP_INT_SIZE,
    'zts' => defined('PHP_ZTS') ? (bool) PHP_ZTS : false,
    'debug' => (bool) PHP_DEBUG,
    'ipv6' => defined('AF_INET6') || @inet_pton('::') !== false,
    'hhvm_version' => defined('HHVM_VERSION') ? HHVM_VERSION : null,
    'extensions' => $exts,
    'timezonedb_loaded' => extension_loaded('timezonedb'),
];

if (extension_loaded('amqp')) {
    $info = ext_info('amqp');
    $out['amqp_librabbitmq'] = grab('/^librabbitmq version => (?<version>.+)$/im', $info);
    $protocol = grab('/^AMQP protocol version => (?<version>.+)$/im', $info);
    $out['amqp_protocol'] = $protocol === null ? null : str_replace('-', '.', $protocol);
}

if (extension_loaded('bz2')) {
    $out['bz2_version'] = grab('/^BZip2 Version => (?<version>.*),/im', ext_info('bz2'));
}

if (extension_loaded('curl')) {
    $cv = curl_version();
    $out['curl_version'] = $cv['version'] ?? null;
    $info = ext_info('curl');
    if (preg_match('{^SSL Version => (?<library>[^/]+)/(?<version>.+)$}im', $info, $m) === 1) {
        $out['curl_ssl_library'] = $m['library'];
        $out['curl_ssl_version'] = $m['version'];
    }
    if (preg_match('{^libSSH Version => (?<library>[^/]+)/(?<version>.+?)(?:/.*)?$}im', $info, $m) === 1) {
        $out['curl_ssh_library'] = $m['library'];
        $out['curl_ssh_version'] = $m['version'];
    }
    $out['curl_zlib_version'] = grab('{^ZLib Version => (?<version>.+)$}im', $info);
}

if (extension_loaded('date')) {
    $info = ext_info('date');
    $out['date_timelib'] = grab('/^timelib version => (?<version>.+)$/im', $info);
    if (preg_match('/^Timezone Database => (?<source>internal|external)$/im', $info, $m) === 1) {
        $out['date_zoneinfo_external'] = $m['source'] === 'external';
        if (preg_match('/^"Olson" Timezone Database Version => (?<version>.+?)(?:\.system)?$/im', $info, $zm) === 1) {
            $out['date_zoneinfo_version'] = $zm['version'];
        }
    }
}

if (extension_loaded('fileinfo')) {
    $out['fileinfo_libmagic'] = grab('/^libmagic => (?<version>.+)$/im', ext_info('fileinfo'));
}

if (extension_loaded('gd')) {
    $out['gd_version'] = defined('GD_VERSION') ? GD_VERSION : null;
    $info = ext_info('gd');
    $out['gd_libjpeg'] = grab('/^libJPEG Version => (?<version>.+?)(?: compatible)?$/im', $info);
    $out['gd_libpng'] = grab('/^libPNG Version => (?<version>.+)$/im', $info);
    $out['gd_freetype'] = grab('/^FreeType Version => (?<version>.+)$/im', $info);
    if (preg_match('/^libXpm Version => (?<version>\d+)$/im', $info, $m) === 1) {
        $out['gd_libxpm_id'] = (int) $m['version'];
    }
}

if (extension_loaded('gmp')) {
    $out['gmp_version'] = defined('GMP_VERSION') ? GMP_VERSION : null;
}

if (extension_loaded('iconv')) {
    $out['iconv_version'] = defined('ICONV_VERSION') ? ICONV_VERSION : null;
}

if (extension_loaded('intl')) {
    $info = ext_info('intl');
    if (defined('INTL_ICU_VERSION')) {
        $out['icu_version'] = INTL_ICU_VERSION;
    } else {
        $out['icu_version'] = grab('/^ICU version => (?<version>.+)$/im', $info);
    }
    $out['icu_tzdata'] = grab('/^ICU TZData version => (?<version>.*)$/im', $info);
    if (class_exists('ResourceBundle', false)) {
        $rb = @ResourceBundle::create('root', 'ICUDATA', false);
        if ($rb !== null) {
            $out['icu_cldr'] = $rb->get('Version');
        }
    }
    if (class_exists('IntlChar', false)) {
        $out['icu_unicode'] = implode('.', array_slice(IntlChar::getUnicodeVersion(), 0, 3));
    }
}

if (extension_loaded('imagick')) {
    try {
        $v = (new Imagick())->getVersion();
        $out['imagick_version_string'] = $v['versionString'] ?? null;
    } catch (\Throwable $e) {
    }
}

if (extension_loaded('ldap')) {
    $info = ext_info('ldap');
    if (preg_match('/^Vendor Version => (?<version>\d+)$/im', $info, $m) === 1
        && preg_match('/^Vendor Name => (?<vendor>.+)$/im', $info, $vm) === 1) {
        $out['ldap_vendor_version_id'] = (int) $m['version'];
        $out['ldap_vendor_name'] = $vm['vendor'];
    }
}

if (extension_loaded('libxml')) {
    $out['libxml_version'] = defined('LIBXML_DOTTED_VERSION') ? LIBXML_DOTTED_VERSION : null;
    $out['libxml_provides'] = array_values(array_intersect($loaded, ['dom', 'simplexml', 'xml', 'xmlreader', 'xmlwriter']));
}

if (extension_loaded('mbstring')) {
    $info = ext_info('mbstring');
    $out['mbstring_libmbfl'] = grab('/^libmbfl version => (?<version>.+)$/im', $info);
    if (defined('MB_ONIGURUMA_VERSION')) {
        $out['mbstring_oniguruma'] = MB_ONIGURUMA_VERSION;
    } else {
        $out['mbstring_oniguruma'] = grab('/^(?:oniguruma|Multibyte regex \(oniguruma\)) version => (?<version>.+)$/im', $info);
    }
}

if (extension_loaded('memcached')) {
    $out['memcached_libmemcached'] = grab('/^libmemcached version => (?<version>.+)$/im', ext_info('memcached'));
}

if (extension_loaded('mongodb')) {
    $info = ext_info('mongodb');
    $out['mongodb_libmongoc'] = grab('/^libmongoc bundled version => (?<version>.+)$/im', $info);
    $out['mongodb_libbson'] = grab('/^libbson bundled version => (?<version>.+)$/im', $info);
}

if (extension_loaded('openssl')) {
    $out['openssl_version_text'] = defined('OPENSSL_VERSION_TEXT') ? OPENSSL_VERSION_TEXT : null;
}

if (extension_loaded('pcre')) {
    $out['pcre_version'] = defined('PCRE_VERSION') ? PCRE_VERSION : null;
    $out['pcre_unicode'] = grab('/^PCRE Unicode Version => (?<version>.+)$/im', ext_info('pcre'));
}

foreach (['mysqlnd', 'pdo_mysql'] as $name) {
    if (!extension_loaded($name)) {
        continue;
    }
    if (preg_match('/^(?:Client API version|Version) => mysqlnd (?<version>.+?) /mi', ext_info($name), $m) === 1) {
        $out[$name . '_mysqlnd'] = $m['version'];
    }
}

if (extension_loaded('pgsql')) {
    if (defined('PGSQL_LIBPQ_VERSION')) {
        $out['pgsql_libpq'] = PGSQL_LIBPQ_VERSION;
    } else {
        $out['pgsql_libpq_info'] = grab('/^PostgreSQL\(libpq\) Version => (?<version>.*)$/im', ext_info('pgsql'));
    }
}
if (extension_loaded('pdo_pgsql')) {
    $out['pdo_pgsql_libpq'] = grab('/^PostgreSQL\(libpq\) Version => (?<version>.*)$/im', ext_info('pdo_pgsql'));
}

if (extension_loaded('pq')) {
    if (preg_match('/^libpq => (?<compiled>.+) => (?<version>.+)$/im', ext_info('pq'), $m) === 1) {
        $out['pq_libpq_linked'] = $m['version'];
    }
}

if (extension_loaded('rdkafka') && defined('RD_KAFKA_VERSION')) {
    $out['rdkafka_version_int'] = RD_KAFKA_VERSION;
}

if ((extension_loaded('libsodium') || extension_loaded('sodium')) && defined('SODIUM_LIBRARY_VERSION')) {
    $out['sodium_library_version'] = SODIUM_LIBRARY_VERSION;
}

foreach (['sqlite3', 'pdo_sqlite'] as $name) {
    if (!extension_loaded($name)) {
        continue;
    }
    $out[$name . '_sqlite'] = grab('/^SQLite Library => (?<version>.+)$/im', ext_info($name));
}

if (extension_loaded('ssh2')) {
    $out['ssh2_libssh2'] = grab('/^libssh2 version => (?<version>.+)$/im', ext_info('ssh2'));
}

if (extension_loaded('xsl')) {
    $out['xsl_libxslt'] = defined('LIBXSLT_DOTTED_VERSION') ? LIBXSLT_DOTTED_VERSION : null;
    $out['xsl_libxslt_libxml'] = grab('/^libxslt compiled against libxml Version => (?<version>.+)$/im', ext_info('xsl'));
}

if (extension_loaded('yaml')) {
    $out['yaml_libyaml'] = grab('/^LibYAML Version => (?<version>.+)$/im', ext_info('yaml'));
}

if (extension_loaded('zip') && defined('ZipArchive::LIBZIP_VERSION')) {
    $out['zip_libzip'] = ZipArchive::LIBZIP_VERSION;
}

if (extension_loaded('zlib')) {
    if (defined('ZLIB_VERSION')) {
        $out['zlib_version'] = ZLIB_VERSION;
    } else {
        $out['zlib_version'] = grab('/^Linked Version => (?<version>.+)$/im', ext_info('zlib'));
    }
}

echo json_encode($out);
"#;

fn run_probe() -> Option<Probe> {
    // No `-r`/`-f -` argument: this build's CLI SAPI treats a literal `-`
    // as a filename ("Could not open input file: -") rather than "read the
    // script from stdin", but bare `php` with redirected stdin and no
    // arguments does read the script from there.
    let mut child = Command::new("php")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()?
        .write_all(PROBE_SCRIPT.as_bytes())
        .ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// "" => 0, "a" => 1, "zg" => 33 (`Version::convertAlphaVersionToIntVersion`).
fn alpha_to_int(alpha: &str) -> i64 {
    alpha
        .bytes()
        .map(|b| i64::from(b) - i64::from(b'a') + 1)
        .sum()
}

/// `Version::parseOpenssl`: OpenSSL's own `1.2.3a`-style versioning below
/// 3.0.0 (patch letter folded into a fourth numeric component via
/// `alpha_to_int`), semver-style above it. `is_fips` is an out-param like
/// the PHP source's `&$isFips` (patch's caller needs it to decide the extra
/// `-fips`-suffixed package/provide).
fn parse_openssl(version: &str) -> Option<(String, bool)> {
    let re = Regex::new(
        r"^(?P<version>[0-9.]+)(?P<patch>[a-z]{0,2})(?P<suffix>(?:-?(?:dev|pre|alpha|beta|rc|fips)[0-9]*)*)(?:-\w+)?(?: \(.+?\))?$",
    )
    .unwrap();
    let caps = re.captures(version)?;
    let numeric = &caps["version"];
    let patch = &caps["patch"];
    let suffix = &caps["suffix"];

    let is_fips = suffix.contains("fips");
    let major_below_3 = numeric
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|major| major < 3);
    let patch_component = if major_below_3 {
        format!(".{}", alpha_to_int(patch))
    } else {
        String::new()
    };
    let suffix = format!("-{}", suffix.trim_start_matches('-'))
        .replace("-fips", "")
        .replace("-pre", "-alpha");
    let out = format!("{numeric}{patch_component}{suffix}");
    Some((out.trim_end_matches('-').to_string(), is_fips))
}

/// `Version::parseLibjpeg`: `"62"` => `"6.2"`, `"80b"` => `"8.2"`... i.e.
/// major digits, dot, `alpha_to_int` of the trailing letters.
fn parse_libjpeg(version: &str) -> Option<String> {
    let re = Regex::new(r"^(?P<major>\d+)(?P<minor>[a-z]*)$").unwrap();
    let caps = re.captures(version)?;
    Some(format!(
        "{}.{}",
        &caps["major"],
        alpha_to_int(&caps["minor"])
    ))
}

/// `Version::parseZoneinfoVersion`: `"2019c"` => `"2019.3"`.
fn parse_zoneinfo(version: &str) -> Option<String> {
    let re = Regex::new(r"^(?P<year>\d{4})(?P<revision>[a-z]*)$").unwrap();
    let caps = re.captures(version)?;
    Some(format!(
        "{}.{}",
        &caps["year"],
        alpha_to_int(&caps["revision"])
    ))
}

/// `Version::convertLibxpmVersionId`/`convertOpenldapVersionId`: both call
/// the same private `convertVersionId($id, 100)` (hex `MMmmrrxx`-adjacent
/// but base-100, not base-16 or base-256).
fn convert_version_id(id: i64) -> String {
    format!("{}.{}.{}", id / 10_000, (id / 100) % 100, id % 100)
}

/// `Version::parseOpenssl` reads `OPENSSL_VERSION_TEXT`/curl's `SSL Version`
/// info line through one more regex first, to strip the leading
/// `OpenSSL `/`LibreSSL ` label before the version-parsing regex above ever
/// sees it (`case 'openssl':`'s own `Preg::isMatchStrictGroups`).
fn strip_ssl_label(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)^(?:OpenSSL|LibreSSL)?\s*(?P<version>\S+)").unwrap();
    re.captures(text).map(|c| c["version"].to_string())
}

/// `PlatformRepository::addExtension`'s normalize-with-fallback: on a
/// version string `VersionParser::normalize` can't parse, Composer keeps
/// only the leading `\d+.\d+.\d+(.\d+)?` (or `"0"` if even that fails) as
/// the package's own pretty version, dropping everything else including
/// the original text (`ext-mysqlnd`'s `phpversion()` return is the classic
/// case — `composer show --platform` prints it as version `0`).
fn extension_pretty_version(raw: &str) -> String {
    if semver::normalize(raw).is_ok() {
        return raw.to_string();
    }
    let re = Regex::new(r"^\d+\.\d+\.\d+(?:\.\d+)?").unwrap();
    re.find(raw)
        .map_or_else(|| "0".to_string(), |m| m.as_str().to_string())
}

/// `PlatformRepository::addLibrary`: silently drops the library if
/// `prettyVersion` is `None`/fails to normalise (both are shrugged off in
/// the PHP source — a library the host's PHP build can't report a clean
/// version for just doesn't become a platform package) or if `lib-{name}`
/// was already added by an earlier, conflicting extension.
fn add_library(
    entries: &mut Vec<Entry>,
    seen: &mut std::collections::HashSet<String>,
    name: &str,
    pretty_version: Option<&str>,
    replaces: &[&str],
    provides: &[&str],
) {
    let Some(pretty_version) = pretty_version else {
        return;
    };
    if semver::normalize(pretty_version).is_err() {
        return;
    }
    let full_name = format!("lib-{name}");
    if !seen.insert(full_name.clone()) {
        return;
    }
    entries.push(Entry {
        name: full_name,
        pretty_version: pretty_version.to_string(),
        provides: provides.iter().map(|p| format!("lib-{p}")).collect(),
        replaces: replaces.iter().map(|r| format!("lib-{r}")).collect(),
    });
}

/// `PlatformRepository::initialize`'s library-scanning loop, minus
/// extension detection itself (the caller already knows which extensions
/// are loaded from `probe.extensions`' keys) — this only turns the probe's
/// already-scraped raw strings into `lib-*` entries, applying exactly the
/// version math `Platform/Version.php` does.
fn library_entries(probe: &Probe) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let loaded = |name: &str| probe.extensions.contains_key(name);

    if loaded("amqp") {
        add_library(
            &mut entries,
            &mut seen,
            "amqp-librabbitmq",
            probe.amqp_librabbitmq.as_deref(),
            &[],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "amqp-protocol",
            probe.amqp_protocol.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("bz2") {
        add_library(
            &mut entries,
            &mut seen,
            "bz2",
            probe.bz2_version.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("curl") {
        add_library(
            &mut entries,
            &mut seen,
            "curl",
            probe.curl_version.as_deref(),
            &[],
            &[],
        );
        if let (Some(library), Some(version)) = (
            probe.curl_ssl_library.as_deref(),
            probe.curl_ssl_version.as_deref(),
        ) {
            let lower = library.to_ascii_lowercase();
            if lower == "openssl" {
                if let Some((parsed, is_fips)) = parse_openssl(version) {
                    let name = if is_fips {
                        "curl-openssl-fips"
                    } else {
                        "curl-openssl"
                    };
                    let desc_version = parsed.clone();
                    add_library(
                        &mut entries,
                        &mut seen,
                        name,
                        Some(&desc_version),
                        &[],
                        if is_fips { &["curl-openssl"] } else { &[] },
                    );
                }
            } else if let Some(rest) = lower.strip_prefix("(securetransport) ") {
                // `case 'curl':`'s own `$shortlib = 'securetransport'` is a
                // constant, only `$sslLib`'s target reads the matched word.
                let matched: String = rest
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                if !matched.is_empty() {
                    add_library(
                        &mut entries,
                        &mut seen,
                        "curl-securetransport",
                        Some(version),
                        &[&format!("curl-{matched}")],
                        &[],
                    );
                }
            } else {
                add_library(
                    &mut entries,
                    &mut seen,
                    &format!("curl-{lower}"),
                    Some(version),
                    &["curl-openssl"],
                    &[],
                );
            }
        }
        if let Some(version) = probe.curl_ssh_version.as_deref() {
            let library = probe
                .curl_ssh_library
                .as_deref()
                .unwrap_or("libssh2")
                .to_ascii_lowercase();
            add_library(
                &mut entries,
                &mut seen,
                &format!("curl-{library}"),
                Some(version),
                &[],
                &[],
            );
        }
        add_library(
            &mut entries,
            &mut seen,
            "curl-zlib",
            probe.curl_zlib_version.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("date") {
        add_library(
            &mut entries,
            &mut seen,
            "date-timelib",
            probe.date_timelib.as_deref(),
            &[],
            &[],
        );
        if let Some(version) = probe.date_zoneinfo_version.as_deref() {
            // No `parse_zoneinfo` here, unlike `icu-zoneinfo` below: `case
            // 'date':` hands `addLibrary` the matched "Olson" Timezone
            // Database Version raw, already `YYYY.N`-shaped on a modern
            // build — only ICU's own TZData version is letter-suffixed
            // (`"2019c"`) and needs that conversion.
            let external = probe.date_zoneinfo_external.unwrap_or(false);
            if external && loaded("timezonedb") {
                add_library(
                    &mut entries,
                    &mut seen,
                    "timezonedb-zoneinfo",
                    Some(version),
                    &["date-zoneinfo"],
                    &[],
                );
            } else {
                add_library(
                    &mut entries,
                    &mut seen,
                    "date-zoneinfo",
                    Some(version),
                    &[],
                    &[],
                );
            }
        }
    }

    if loaded("fileinfo") {
        add_library(
            &mut entries,
            &mut seen,
            "fileinfo-libmagic",
            probe.fileinfo_libmagic.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("gd") {
        add_library(
            &mut entries,
            &mut seen,
            "gd",
            probe.gd_version.as_deref(),
            &[],
            &[],
        );
        if let Some(v) = probe.gd_libjpeg.as_deref().and_then(parse_libjpeg) {
            add_library(&mut entries, &mut seen, "gd-libjpeg", Some(&v), &[], &[]);
        }
        add_library(
            &mut entries,
            &mut seen,
            "gd-libpng",
            probe.gd_libpng.as_deref(),
            &[],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "gd-freetype",
            probe.gd_freetype.as_deref(),
            &[],
            &[],
        );
        if let Some(id) = probe.gd_libxpm_id {
            let v = convert_version_id(id);
            add_library(&mut entries, &mut seen, "gd-libxpm", Some(&v), &[], &[]);
        }
    }

    if loaded("gmp") {
        add_library(
            &mut entries,
            &mut seen,
            "gmp",
            probe.gmp_version.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("iconv") {
        add_library(
            &mut entries,
            &mut seen,
            "iconv",
            probe.iconv_version.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("intl") {
        add_library(
            &mut entries,
            &mut seen,
            "icu",
            probe.icu_version.as_deref(),
            &[],
            &[],
        );
        if let Some(v) = probe.icu_tzdata.as_deref().and_then(parse_zoneinfo) {
            add_library(&mut entries, &mut seen, "icu-zoneinfo", Some(&v), &[], &[]);
        }
        add_library(
            &mut entries,
            &mut seen,
            "icu-cldr",
            probe.icu_cldr.as_deref(),
            &[],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "icu-unicode",
            probe.icu_unicode.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("imagick")
        && let Some(text) = probe.imagick_version_string.as_deref()
    {
        let re = Regex::new(r"^ImageMagick (?P<version>[\d.]+)(?:-(?P<patch>\d+))?").unwrap();
        if let Some(caps) = re.captures(text) {
            let mut version = caps["version"].to_string();
            if let Some(patch) = caps.name("patch") {
                version.push('.');
                version.push_str(patch.as_str());
            }
            add_library(
                &mut entries,
                &mut seen,
                "imagick-imagemagick",
                Some(&version),
                &["imagick"],
                &[],
            );
        }
    }

    if loaded("ldap")
        && let (Some(id), Some(vendor)) = (
            probe.ldap_vendor_version_id,
            probe.ldap_vendor_name.as_deref(),
        )
    {
        let version = convert_version_id(id);
        add_library(
            &mut entries,
            &mut seen,
            &format!("ldap-{}", vendor.to_ascii_lowercase()),
            Some(&version),
            &[],
            &[],
        );
    }

    if loaded("libxml") {
        let provides: Vec<&str> = probe.libxml_provides.iter().map(String::as_str).collect();
        let provides: Vec<String> = provides.iter().map(|e| format!("{e}-libxml")).collect();
        let provides_refs: Vec<&str> = provides.iter().map(String::as_str).collect();
        add_library(
            &mut entries,
            &mut seen,
            "libxml",
            probe.libxml_version.as_deref(),
            &[],
            &provides_refs,
        );
    }

    if loaded("mbstring") {
        add_library(
            &mut entries,
            &mut seen,
            "mbstring-libmbfl",
            probe.mbstring_libmbfl.as_deref(),
            &[],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "mbstring-oniguruma",
            probe.mbstring_oniguruma.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("memcached") {
        add_library(
            &mut entries,
            &mut seen,
            "memcached-libmemcached",
            probe.memcached_libmemcached.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("mongodb") {
        add_library(
            &mut entries,
            &mut seen,
            "mongodb-libmongoc",
            probe.mongodb_libmongoc.as_deref(),
            &[],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "mongodb-libbson",
            probe.mongodb_libbson.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("openssl")
        && let Some(stripped) = probe
            .openssl_version_text
            .as_deref()
            .and_then(strip_ssl_label)
        && let Some((parsed, is_fips)) = parse_openssl(&stripped)
    {
        let name = if is_fips { "openssl-fips" } else { "openssl" };
        add_library(
            &mut entries,
            &mut seen,
            name,
            Some(&parsed),
            &[],
            if is_fips { &["openssl"] } else { &[] },
        );
    }

    if loaded("pcre") {
        if let Some(raw) = probe.pcre_version.as_deref() {
            // `case 'pcre':` keeps only the first whitespace-delimited
            // token (`PCRE_VERSION` is `"10.42 2022-12-11"`-shaped).
            let first = raw.split_whitespace().next().unwrap_or(raw);
            add_library(&mut entries, &mut seen, "pcre", Some(first), &[], &[]);
        }
        add_library(
            &mut entries,
            &mut seen,
            "pcre-unicode",
            probe.pcre_unicode.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("mysqlnd") {
        add_library(
            &mut entries,
            &mut seen,
            "mysqlnd-mysqlnd",
            probe.mysqlnd_mysqlnd.as_deref(),
            &[],
            &[],
        );
    }
    if loaded("pdo_mysql") {
        add_library(
            &mut entries,
            &mut seen,
            "pdo_mysql-mysqlnd",
            probe.pdo_mysql_mysqlnd.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("pgsql") {
        if let Some(v) = probe.pgsql_libpq.as_deref() {
            add_library(&mut entries, &mut seen, "pgsql-libpq", Some(v), &[], &[]);
        } else {
            add_library(
                &mut entries,
                &mut seen,
                "pgsql-libpq",
                probe.pgsql_libpq_info.as_deref(),
                &[],
                &[],
            );
        }
    }
    if loaded("pdo_pgsql") {
        add_library(
            &mut entries,
            &mut seen,
            "pdo_pgsql-libpq",
            probe.pdo_pgsql_libpq.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("pq") {
        add_library(
            &mut entries,
            &mut seen,
            "pq-libpq",
            probe.pq_libpq_linked.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("rdkafka")
        && let Some(v) = probe.rdkafka_version_int
    {
        let version = format!(
            "{}.{}.{}",
            (v & 0x7F00_0000) >> 24,
            (v & 0x00FF_0000) >> 16,
            (v & 0x0000_FF00) >> 8
        );
        add_library(
            &mut entries,
            &mut seen,
            "rdkafka-librdkafka",
            Some(&version),
            &[],
            &[],
        );
    }

    if loaded("libsodium") || loaded("sodium") {
        add_library(
            &mut entries,
            &mut seen,
            "libsodium",
            probe.sodium_library_version.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("sqlite3") {
        add_library(
            &mut entries,
            &mut seen,
            "sqlite3-sqlite",
            probe.sqlite3_sqlite.as_deref(),
            &[],
            &[],
        );
    }
    if loaded("pdo_sqlite") {
        add_library(
            &mut entries,
            &mut seen,
            "pdo_sqlite-sqlite",
            probe.pdo_sqlite_sqlite.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("ssh2") {
        add_library(
            &mut entries,
            &mut seen,
            "ssh2-libssh2",
            probe.ssh2_libssh2.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("xsl") {
        add_library(
            &mut entries,
            &mut seen,
            "libxslt",
            probe.xsl_libxslt.as_deref(),
            &["xsl"],
            &[],
        );
        add_library(
            &mut entries,
            &mut seen,
            "libxslt-libxml",
            probe.xsl_libxslt_libxml.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("yaml") {
        add_library(
            &mut entries,
            &mut seen,
            "yaml-libyaml",
            probe.yaml_libyaml.as_deref(),
            &[],
            &[],
        );
    }

    if loaded("zip") {
        add_library(
            &mut entries,
            &mut seen,
            "zip-libzip",
            probe.zip_libzip.as_deref(),
            &["zip"],
            &[],
        );
    }

    if loaded("zlib") {
        add_library(
            &mut entries,
            &mut seen,
            "zlib",
            probe.zlib_version.as_deref(),
            &[],
            &[],
        );
    }

    entries
}

/// `PlatformRepository::addExtension`: one `ext-*` entry per loaded
/// extension (skipping `core`/`standard`, already excluded from
/// `probe.extensions`' keys by the probe script itself), `ext-uuid`
/// additionally replacing `lib-uuid` (the one extension with a bespoke
/// link outside the library switch).
fn extension_entries(probe: &Probe) -> Vec<Entry> {
    probe
        .extensions
        .iter()
        .map(|(name, version)| {
            let raw = version.as_str().unwrap_or("0");
            let package_name = format!("ext-{}", name.to_ascii_lowercase().replace(' ', "-"));
            let mut entry = Entry::new(package_name, extension_pretty_version(raw));
            if name.eq_ignore_ascii_case("uuid") {
                entry.replaces.push("lib-uuid".to_string());
            }
            entry
        })
        .collect()
}

/// `Runtime::getConstant('PHP_VERSION')`'s own try/catch: a version
/// `VersionParser::normalize` can't parse falls back to everything before
/// the first `~`/`+`/`-`, matching `PlatformRepository::initialize`'s
/// top-level `php` package (not `addExtension`'s digits-only fallback,
/// which only the `ext-*`/`php-*` companions after it would ever hit via
/// sharing this same pretty version).
fn php_pretty_version(raw: &str) -> String {
    if semver::normalize(raw).is_ok() {
        return raw.to_string();
    }
    let re = Regex::new(r"^[^~+-]+").unwrap();
    re.find(raw)
        .map_or_else(|| raw.to_string(), |m| m.as_str().to_string())
}

/// `PlatformRepository::initialize`'s `php`/`php-debug`/`php-zts`/
/// `php-64bit`/`php-ipv6` block, all sharing one detected version.
fn php_entries(probe: &Probe) -> Vec<Entry> {
    let pretty = php_pretty_version(&probe.php_version);
    let mut entries = vec![Entry::new("php", pretty.clone())];
    if probe.debug {
        entries.push(Entry::new("php-debug", pretty.clone()));
    }
    if probe.zts {
        entries.push(Entry::new("php-zts", pretty.clone()));
    }
    if probe.int_size == 8 {
        entries.push(Entry::new("php-64bit", pretty.clone()));
    }
    if probe.ipv6 {
        entries.push(Entry::new("php-ipv6", pretty));
    }
    entries
}

/// `PlatformRepository::initialize`'s fixed platform packages
/// (`platform_packages`, `detect_php_version` and `detect_extensions` are
/// this function's pre-port names): `php`/`php-*`, `composer-plugin-api`,
/// `composer-runtime-api`, one `ext-*` per loaded extension and one
/// `lib-*` per library `PlatformRepository::initialize`'s switch detects,
/// each with the exact pretty version Composer would report (cross-check:
/// `composer show --platform`) — plus their `replace`/`provide` links
/// (`curl-openssl`, `libxml`'s `*-libxml` provides, `ext-uuid`'s
/// `lib-uuid` replace, ...).
///
/// `overrides` is `config.platform` verbatim (`Installer::doUpdate`'s
/// `$this->config->get('platform')`); see the module doc comment for its
/// semantics.
pub(crate) fn platform_packages(overrides: &Map<String, Value>) -> Result<Vec<Package>> {
    let probe = run_probe();

    let mut entries = vec![
        Entry::new("composer-plugin-api", "2.9.0"),
        Entry::new("composer-runtime-api", "2.2.2"),
    ];

    if let Some(probe) = &probe {
        entries.extend(php_entries(probe));
        entries.extend(extension_entries(probe));
        entries.extend(library_entries(probe));
        if let Some(hhvm) = probe.hhvm_version.as_deref() {
            entries.push(Entry::new("hhvm", php_pretty_version(hhvm)));
        }
    } else {
        entries.push(Entry::new("php", "8.3.0"));
    }

    for (name, value) in overrides {
        match value {
            Value::String(version) => {
                if let Some(entry) = entries.iter_mut().find(|e| &e.name == name) {
                    entry.pretty_version.clone_from(version);
                    entry.provides.clear();
                    entry.replaces.clear();
                } else {
                    entries.push(Entry::new(name.clone(), version.clone()));
                }
            }
            Value::Bool(false) => entries.retain(|e| &e.name != name),
            _ => {}
        }
    }

    entries.into_iter().map(entry_to_package).collect()
}

fn entry_to_package(entry: Entry) -> Result<Package> {
    let version = semver::normalize(&entry.pretty_version)?;
    let link = |target: String| -> Result<Link> {
        Ok(Link {
            constraint: Some(Arc::new(semver::parse_constraint(&format!(
                "={}",
                version.as_str()
            ))?)),
            pretty_constraint: Some(entry.pretty_version.clone()),
            target,
        })
    };
    let provides = entry
        .provides
        .into_iter()
        .map(link)
        .collect::<Result<_>>()?;
    let replaces = entry
        .replaces
        .into_iter()
        .map(link)
        .collect::<Result<_>>()?;

    Ok(Package {
        stability: semver::stability(version.as_str()),
        is_dev: false,
        version,
        pretty_version: entry.pretty_version,
        requires: Vec::new(),
        conflicts: Vec::new(),
        provides,
        replaces,
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw: Arc::new(serde_json::json!({ "name": entry.name, "version": Value::Null })),
        name: entry.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe JSON recorded from this host's devbox `php` (`php --version`
    /// reported 8.4.24, cross-checked field for field against `devbox run
    /// -- composer show --platform`'s own output for every name both
    /// produce).
    const RECORDED_PROBE: &str = include_str!("../../tests/fixtures/platform/probe.json");

    fn recorded() -> Probe {
        serde_json::from_str(RECORDED_PROBE).unwrap()
    }

    fn packages(probe: &Probe) -> Vec<Package> {
        let mut entries = php_entries(probe);
        entries.extend(extension_entries(probe));
        entries.extend(library_entries(probe));
        entries
            .into_iter()
            .map(entry_to_package)
            .collect::<Result<_>>()
            .unwrap()
    }

    fn find<'a>(packages: &'a [Package], name: &str) -> &'a Package {
        packages
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} missing from platform_packages"))
    }

    /// #118: the recorded host's real `php-64bit`, `lib-pcre`, `lib-icu`
    /// and `lib-openssl` versions, matching `composer show --platform`.
    #[test]
    fn recorded_probe_produces_composer_compatible_versions() {
        let probe = recorded();
        let packages = packages(&probe);

        assert_eq!(find(&packages, "php-64bit").pretty_version, "8.4.24");
        assert_eq!(find(&packages, "lib-pcre").pretty_version, "10.47");
        assert_eq!(find(&packages, "lib-icu").pretty_version, "73.2");
        assert_eq!(find(&packages, "lib-openssl").pretty_version, "3.6.3");
        assert_eq!(find(&packages, "lib-pcre-unicode").pretty_version, "16.0.0");
        assert_eq!(find(&packages, "php-ipv6").pretty_version, "8.4.24");
        // #118's regression: `date-zoneinfo`'s raw phpinfo value is already
        // dotted (unlike ICU's letter-suffixed TZData), so it must not go
        // through `parse_zoneinfo`.
        assert_eq!(
            find(&packages, "lib-date-zoneinfo").pretty_version,
            "2026.3"
        );
    }

    #[test]
    fn openssl_below_3_folds_the_patch_letter() {
        let (version, is_fips) = parse_openssl("1.1.1t").unwrap();
        assert_eq!(version, "1.1.1.20");
        assert!(!is_fips);
    }

    #[test]
    fn openssl_fips_suffix_is_detected_and_stripped() {
        let (version, is_fips) = parse_openssl("3.0.0-fips").unwrap();
        assert_eq!(version, "3.0.0");
        assert!(is_fips);
    }

    #[test]
    fn libjpeg_alpha_suffix_becomes_a_numeric_minor() {
        assert_eq!(parse_libjpeg("80b").unwrap(), "80.2");
    }

    #[test]
    fn zoneinfo_revision_letter_becomes_a_numeric_minor() {
        assert_eq!(parse_zoneinfo("2019c").unwrap(), "2019.3");
    }

    #[test]
    fn version_id_matches_the_documented_example() {
        assert_eq!(convert_version_id(20_607), "2.6.7");
    }
}
