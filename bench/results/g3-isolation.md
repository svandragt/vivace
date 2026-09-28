# Candidate 3.2 phase A: bundled Composer dependency inventory (#330)

How to reproduce: `make bench-g3-isolation` (network: WordPress.org only). Plugin list fetched 2026-09-28.

Plugins examined: 100 of 100 requested.

## Totals

- Plugins bundling a `vendor/` directory: **46** of 100.
- Of those, with a `vendor/composer/installed.json`: **36**.
- Distinct libraries seen across all bundling plugins: **220**.
- Libraries present in 2 or more plugins: **65**.
- Conflict candidates (library x plugin copy, versions differ across plugins, this copy unprefixed): **157**.
- Library copies by prefixing: yes 19, no 231, unknown 120 (of 370 total copies, 5.1% prefixed).

## Top 10 libraries by number of plugins bundling them

| Library | Plugins | Versions in use |
|---|---|---|
| woocommerce/action-scheduler | 9 | 4.0.0 (woocommerce); unknown (wpforms-lite, wp-mail-smtp); 3.9.3 (seo-by-rank-math, hostinger, all-in-one-seo-pack, hostinger-reach, imagify); 3.9.0 (image-optimization) |
| psr/log | 8 | 1.1.4 (updraftplus, complianz-gdpr, sg-ai-studio, wp-optimize, google-listings-and-ads, woocommerce-paypal-payments); 1.1.0 (wpvivid-backuprestore); unknown (fluent-smtp) |
| automattic/jetpack-autoloader | 7 | v5.0.23 (elementor, hostinger, hostinger-reach); v5.0.0 (woocommerce); v5.0.15 (really-simple-ssl); v5.0.16 (woocommerce-payments); v5.0.20 (premium-addons-for-elementor) |
| paragonie/random_compat | 7 | unknown (wordfence, wp-mail-smtp); v2.0.21 (updraftplus, wpvivid-backuprestore); v9.99.100 (complianz-gdpr, google-listings-and-ads, mainwp-child) |
| ralouphie/getallheaders | 7 | unknown (wp-mail-smtp, fluent-smtp); 2.0.5 (updraftplus, wpvivid-backuprestore); 3.0.3 (w3-total-cache, google-listings-and-ads, woocommerce-paypal-payments) |
| composer/installers | 6 | v1.12.0 (woocommerce, sg-cachepress, sg-security); v2.3.0 (code-snippets, imagify); v1.10.0 (woocommerce-payments) |
| psr/http-message | 6 | 1.1 (updraftplus); 2.0 (complianz-gdpr, w3-total-cache, google-listings-and-ads); 1.0.1 (wpvivid-backuprestore); unknown (fluent-smtp) |
| psr/container | 6 | dev-master (hostinger-reach); 1.1.2 (sg-ai-studio, instagram-feed, google-listings-and-ads); 2.0.2 (wp-optimize, imagify) |
| wordpress/mcp-adapter | 5 | v0.6.1 (elementor); v0.3.0 (woocommerce); v0.5.0 (seo-by-rank-math, imagify, premium-addons-for-elementor) |
| guzzlehttp/guzzle | 5 | 6.5.8 (updraftplus); 6.3.3 (wpvivid-backuprestore); 7.15.5 (w3-total-cache, google-listings-and-ads); unknown (fluent-smtp) |

## Reading

Of the 100 plugins examined, 46 bundle a vendor/ directory of their own, and 36 of those carry a vendor/composer/installed.json Composer itself wrote. Together they bundle 220 distinct libraries; 65 of those turn up in two or more of the sampled plugins, most of them at different version strings -- exactly the setup PHP's one global namespace can't tell apart at runtime. The most-repeated library, `woocommerce/action-scheduler`, turns up in 9 of them. Reading each bundled library's own PHP files, 19 of the 370 bundled copies already carry a foreign namespace prefix (php-scoper/Strauss/Mozart style), 231 keep the library's own unprefixed namespace (or, for old-style code with no namespaces at all, its own unprefixed class names), and 120 couldn't be read either way from the files sampled. 157 library-plugin pairs combine a version that disagrees with at least one other plugin's copy of the same library and a copy this script couldn't confirm is prefixed -- the runtime collision candidates.

Wall time: 1.8s.

