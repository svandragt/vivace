<?php
// #352's load check (`src/isolate.rs`'s `boot_check`): the minimal set of
// `WordPress` globals php-scoper's own `exclude-functions`/`exclude-classes`
// config assumes are already defined when a scoped plugin's main file loads
// stand-alone, outside a real `WordPress` bootstrap. Extend this file the
// first time a real plugin's load check needs a stub it doesn't define yet.

if (!defined('ABSPATH')) {
    define('ABSPATH', __DIR__ . '/');
}

if (!function_exists('add_action')) {
    function add_action(...$args) {
        return true;
    }
}

if (!function_exists('add_filter')) {
    function add_filter(...$args) {
        return true;
    }
}

if (!function_exists('do_action')) {
    function do_action(...$args) {
    }
}

if (!function_exists('apply_filters')) {
    function apply_filters($tag, $value = null, ...$args) {
        return $value;
    }
}

if (!function_exists('plugin_dir_path')) {
    function plugin_dir_path($file) {
        return rtrim(dirname($file), '/\\') . '/';
    }
}

if (!function_exists('plugin_dir_url')) {
    function plugin_dir_url($file) {
        return '';
    }
}

if (!function_exists('__')) {
    function __($text, $domain = 'default') {
        return $text;
    }
}

if (!function_exists('esc_html')) {
    function esc_html($text) {
        return $text;
    }
}

if (!function_exists('wp_die')) {
    function wp_die($message = '') {
        throw new RuntimeException(is_string($message) ? $message : 'wp_die called');
    }
}
