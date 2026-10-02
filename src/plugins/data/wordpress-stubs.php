<?php
// #352/#357's load check (`src/isolate.rs`'s `boot_check`): the real
// `php-stubs/wordpress-stubs` package (resolved through the tool
// environment, same path as php-scoper) now covers every `WordPress` core
// function/class a scoped plugin's main file calls — this file is only for
// what that package leaves undefined: `ABSPATH` is a constant WP's own
// `wp-load.php` defines at runtime, never a stub generator's concern.
// Extend this file the first time a real plugin's load check needs
// something else the real stubs don't cover.

if (!defined('ABSPATH')) {
    define('ABSPATH', __DIR__ . '/');
}
