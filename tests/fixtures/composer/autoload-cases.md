# Autoload golden cases

Case setups behind the goldens in `autoload/`, extracted from
composer/composer `tests/Composer/Test/Autoload/AutoloadGeneratorTest.php`
(commit 85ae025). Port each as a row in `tests/autoload_goldens.rs`.

Setup shared by all cases: `workingDir` is a fresh temp dir and the cwd;
`vendorDir` is `workingDir/composer-test-autoload` unless stated; config
`platform-check` true, `use-include-path` false; install path of a package is
`vendorDir/<name>` plus its target-dir; metapackages have no install path;
`getDevPackageNames` returns `[]`. `assertAutoloadFiles(name, dir, type)`
compares `autoload/autoload_<name>.php` with `<dir>/autoload_<type>.php`
byte for byte after normalising `\r`. "dump: dev=true" means the
`scanPsrPackages` argument (optimise), "setDevMode(true)" is dev mode.

### testRootPackageAutoloading
- root: `root/a` 1.0; autoload psr-0 {Main→src/, Lala→[src/, lib/]}, psr-4 {Acme\Fruit\→src-fruit/, Acme\Cake\→[src-cake/, lib-cake/]}, classmap [composersrc/]
- files: workingDir/src/Lala/ClassMapMain.php, src/Lala/Test/ClassMapMainTest.php, src-cake/ClassMapBar.php, composersrc/foo.php
- dump: dev=true (scan), suffix `_1`
- asserts: autoload_main → namespaces; autoload_psr4 → psr4; autoload_classmap → classmap

### testRootPackageDevAutoloading
- root: `root/a` 1.0; autoload psr-0 {Main→src/}; autoload-dev files [devfiles/foo.php], psr-0 {Main→tests/}
- files: src/Main/ClassMain.php, devfiles/foo.php
- dump: setDevMode(true), dev=true, suffix `_1`
- asserts: autoload_main5 → namespaces; autoload_classmap7 → classmap; autoload_files2 → files

### testRootPackageDevAutoloadingDisabledByDefault
- same root as above but autoload-dev only files [devfiles/foo.php]; dev mode not set
- dump: dev=true, suffix `_1`
- asserts: autoload_main4 → namespaces; autoload_classmap7 → classmap; autoload_files.php absent

### testVendorDirSameAsWorkingDir
- root: `root/a` 1.0; psr-0 {Main→src/, Lala→src/}, psr-4 {Acme\Fruit\→src-fruit/, Acme\Cake\→[src-cake/, lib-cake/]}, classmap [composersrc/]
- vendorDir = workingDir; files: src/Main/Foo.php, composersrc/foo.php
- dump: dev=true, suffix `_2`
- asserts: autoload_main3 → namespaces; autoload_psr4_3 → psr4; autoload_classmap3 → classmap

### testRootPackageAutoloadingAlternativeVendorDir
- same root autoload; vendorDir = workingDir/subdir; files: composersrc/foo.php
- dump: dev=false, suffix `_3`
- asserts: autoload_main2 → namespaces; autoload_psr4_2 → psr4; autoload_classmap2 → classmap

### testRootPackageAutoloadingWithTargetDir
- root: `root/a` 1.0, target-dir `Main/Foo/`; psr-0 {Main\Foo→'', Main\Bar→''}, classmap [Main/Foo/src, lib], files [foo.php, Main/Foo/bar.php]
- files: src/rootfoo.php, lib/rootbar.php, foo.php, bar.php
- dump: dev=false, suffix `TargetDir`
- asserts: autoload_target_dir → vendor/autoload.php; autoload_real_target_dir → autoload_real; autoload_static_target_dir → autoload_static; autoload_files_target_dir → files; autoload_classmap6 → classmap

### testDuplicateFilesWarning
- root: `root/a` 1.0; files [foo.php, bar.php, ./foo.php, ././foo.php]; files exist
- dump: dev=false, suffix `FilesWarning`
- asserts: autoload_files_duplicates → files; warning "The following "files" autoload rules are included multiple times" mentioning `$baseDir . '/foo.php'`

### testVendorsAutoloading
- root `root/a` requires a/a, b/b; packages a/a 1.0 psr-0 {A→src/, A\B→lib/}; b/b 1.0 psr-0 {B\Sub\Name→src/}; alias b/b as 1.2
- dirs: vendor/a/a/src, a/a/lib, b/b/src
- dump: dev=false, suffix `_5`
- asserts: autoload_vendors → namespaces; classmap file exists (empty map)

### testVendorsAutoloadingWithMetapackages
- as above but a/a is a metapackage requiring b/b (its autoload ignored, no install path)
- asserts: autoload_vendors_meta → namespaces

### testNonDevAutoloadExclusionWithRecursion
- root requires a/a; a/a psr-0 {A→src/, A\B→lib/} requires b/b; b/b psr-0 {B\Sub\Name→src/} requires a/a
- dump: dev=false, suffix `_5`; asserts: autoload_vendors → namespaces

### testNonDevAutoloadShouldIncludeReplacedPackages
- root requires a/a; a/a requires b/c; b/b psr-4 {B\→src/} replaces b/c; file vendor/b/b/src/C/C.php `<?php namespace B\C; class C {}`
- dump: dev=true, suffix `_5`; asserts: classmap contains B\C\C and Composer\InstalledVersions

### testNonDevAutoloadExclusionWithRecursionReplace
- root requires a/a; a/a psr-0 {A→src/, A\B→lib/} requires c/c; b/b psr-0 {B\Sub\Name→src/} replaces c/c
- dump: dev=false, suffix `_5`; asserts: autoload_vendors → namespaces

### testNonDevAutoloadReplacesNestedRequirements
- root requires a/a; a/a classmap [src/A.php] requires b/b; b/b classmap [src/B.php] requires e/e; c/c classmap [src/C.php] replaces b/b requires d/d; d/d classmap [src/D.php]; e/e classmap [src/E.php]; all files exist with one class each
- dump: dev=false, suffix `_5`; asserts: autoload_classmap9 → classmap

### testPharAutoload
- root requires a/a; root psr-0 {Foo→foo.phar, Bar→dir/bar.phar/src}, psr-4 {Baz\→baz.phar, Qux\→dir/qux.phar/src}; a/a psr-0 {Lorem→lorem.phar, Ipsum→dir/ipsum.phar/src}, psr-4 {Dolor\→dolor.phar, Sit\→dir/sit.phar/src}; no files
- dump: dev=true, suffix `Phar`
- asserts: autoload_phar → namespaces; autoload_phar_psr4 → psr4; autoload_phar_static → static

### testPSRToClassMapIgnoresNonExistingDir
- root psr-0 {Prefix→foo/bar/non/existing/}, psr-4 {Prefix\→foo/bar/non/existing2/}
- dump: dev=true, suffix `_8`; asserts: classmap has only Composer\InstalledVersions

### testPSRToClassMapIgnoresNonPSRClasses
- root psr-0 {psr0_→psr0/}, psr-4 {psr4\→psr4/}; files psr0/psr0/match.php `class psr0_match {}`, psr0/psr0/badfile.php `class psr0_badclass {}`, psr4/match.php `namespace psr4; class match {}`, psr4/badfile.php `namespace psr4; class badclass {}`
- dump: dev=true, suffix `_1`; asserts: classmap exactly Composer\InstalledVersions, psr0_match, psr4\match (inline expected)

### testVendorsClassMapAutoloading
- root requires a/a, b/b; a/a classmap [src/]; b/b classmap [src/, lib/]; files vendor/a/a/src/a.php `class ClassMapFoo {}`, b/b/src/b.php `class ClassMapBar {}`, b/b/lib/c.php `class ClassMapBaz {}`
- dump: dev=false, suffix `_6`; asserts: autoload_classmap4 → classmap

### testVendorsClassMapAutoloadingWithTargetDir
- a/a target-dir `target`, classmap [target/src/, lib/]; b/b classmap [src/]; files a/a/target/src/a.php (ClassMapFoo), a/a/target/lib/b.php (ClassMapBar), b/b/src/c.php (ClassMapBaz)
- dump: dev=false, suffix `_6`; asserts: map ClassMapBar→vendor/a/a/target/lib/b.php, ClassMapBaz→vendor/b/b/src/c.php, ClassMapFoo→vendor/a/a/target/src/a.php, plus InstalledVersions

### testClassMapAutoloadingEmptyDirAndExactFile
- a/a classmap ['']; b/b classmap [test.php]; c/c classmap [./]; files a/a/src/a.php (ClassMapFoo), b/b/test.php (ClassMapBar), c/c/foo/test.php (ClassMapBaz)
- dump: dev=false, suffix `_7`; asserts: autoload_classmap5 → classmap; autoload_real has no setClassMapAuthoritative/setApcuPrefix

### testClassMapAutoloadingAuthoritativeAndApcu (+Prefix variant)
- a/a psr-4 {''→src/}; b/b psr-4 {''→./}; c/c psr-4 {''→foo/}; files a/a/src/ClassMapFoo.php, b/b/ClassMapBar.php, c/c/foo/ClassMapBaz.php
- setClassMapAuthoritative(true), setApcu(true[, "custom'Prefix"]); dump dev=false, suffix `_7`
- asserts: autoload_classmap8 → classmap; autoload_real contains `setClassMapAuthoritative(true)` and `setApcuPrefix(` (`setApcuPrefix('custom\'Prefix')` in the prefix variant)

### testFilesAutoloadGeneration
- root files [root.php] requires a/a, b/b, c/c; a/a files [test.php]; b/b files [test2.php]; c/c target-dir foo/bar files [test3.php, foo/bar/test4.php]; files exist (each defines testFilesAutoloadGenerationN())
- dump: dev=false, suffix `FilesAutoload`
- asserts: autoload_functions → vendor/autoload.php; autoload_real_functions → real; autoload_static_functions → static; autoload_files_functions → files

### testFilesAutoloadGenerationRemoveExtraEntitiesFromAutoloadFiles
- as above plus include-paths: root [/lib, /src], a/a [lib1, src1], b/b [lib2], c/c [lib3]
- dump 1: asserts autoload_functions → autoload.php; autoload_real_functions_with_include_paths → real; autoload_static_functions_with_include_paths → static; autoload_files_functions → files; include_paths_functions → include_paths
- dump 2 (same input): autoload_files_functions_with_removed_extra → files; include_paths_functions_with_removed_extra → include_paths
- dump 3 (root with no autoload/include-paths, same requires): autoload_real_functions_with_removed_include_paths_and_autolad_files → real; autoload_static_... → static; files and include_paths absent

### testFilesAutoloadOrderByDependencies
- root files [root2.php] requires z/foo, b/bar, d/d, e/e; z/foo files [testA.php] requires c/lorem; b/bar files [testB.php] requires c/lorem, d/d; c/lorem files [testC.php]; d/d files [testD.php] requires c/lorem; e/e files [testE.php] requires c/lorem
- dump: dev=false, suffix `FilesAutoloadOrder`
- asserts: autoload_functions_by_dependency → autoload.php; autoload_real_files_by_dependency → real; autoload_static_files_by_dependency → static

### testOverrideVendorsAutoloading
- root `root/z` psr-0 {A\B→<workingDir>/lib} (absolute), classmap [<workingDir>/src] requires a/a, b/b; a/a psr-0 {A→src/, A\B→lib/} classmap [classmap]; b/b psr-0 {B\Sub\Name→src/}
- files: workingDir/lib/A/B/C.php, workingDir/src/classes.php (Foo\Bar), vendor/a/a/lib/A/B/C.php, vendor/a/a/classmap/classes.php
- dump: dev=true, suffix `_9`
- asserts (inline): namespaces `'A\\B' => array($baseDir . '/lib', $vendorDir . '/a/a/lib'), 'A' => array($vendorDir . '/a/a/src')`; psr4 empty; classmap A\B\C→$baseDir/lib/A/B/C.php, Composer\InstalledVersions, Foo\Bar→$baseDir/src/classes.php

### testIncludePathFileGeneration / ArePrepended / InRootPackage / WithoutPathsIsSkipped
- include-paths only; include_paths → include_paths.php; absent when no paths. Out of scope for v0.1 unless cheap.

### testUseGlobalIncludePath
- root psr-0 {Main\Foo→'', Main\Bar→''}, target-dir Main/Foo/; config use-include-path true
- dump: dev=false, suffix `IncludePath`; asserts: autoload_real_include_path → real; autoload_static_include_path → static

### testVendorDirExcludedFromWorkingDir
- workingDir = vendorDir/working-dir; root psr-0 {Foo→src}, psr-4 {Acme\Foo\→src-psr4}, classmap [classmap], files [test.php] requires b/b; b/b psr-0 {Bar→lib}, psr-4 {Acme\Bar\→lib-psr4}, classmap [classmaps], files [bootstrap.php]
- files: workingDir/src/Foo/Bar.php, classmap/classes.php, test.php, vendor/b/b/lib/Bar/Foo.php, b/b/classmaps/classes.php, b/b/bootstrap.php
- dump: dev=true, suffix `_13`
- asserts (inline): Foo→$baseDir/src, Bar→$vendorDir/b/b/lib; Acme\Foo\→$baseDir/src-psr4, Acme\Bar\→$vendorDir/b/b/lib-psr4; classmap Bar\Bar, Bar\Foo, Composer\InstalledVersions, Foo\Bar, Foo\Foo; files has $vendorDir/b/b/bootstrap.php and $baseDir/test.php

### testUpLevelRelativePaths
- workingDir = <tmp>/working-dir; root psr-0 {Foo→../path/../src}, psr-4 {Acme\Foo\→../path/../src-psr4}, classmap [../classmap, ../classmap2/subdir, classmap3, classmap4], files [../test.php], exclude-from-classmap [./../classmap/excluded, ../classmap2, classmap3/classes.php, classmap4/*/classes.php]
- files: <tmp>/src/Foo/Bar.php, <tmp>/classmap/classes.php, <tmp>/classmap/excluded/classes.php, <tmp>/classmap2/subdir/classes.php, working-dir/classmap3/classes.php, working-dir/classmap4/foo/classes.php, <tmp>/test.php
- dump: dev=true, suffix `_14`
- asserts (inline): Foo→$baseDir/../src; Acme\Foo\→$baseDir/../src-psr4; classmap Composer\InstalledVersions, Foo\Bar, Foo\Foo; files has $baseDir/../test.php

### testAutoloadRulesInPackageThatDoesNotExistOnDisk
- root requires dep/a; dep/a not on disk. psr-0 {Foo→./src} → Foo→$vendorDir/dep/a/src; psr-4 {Acme\Foo\→./src-psr4} → $vendorDir/dep/a/src-psr4; classmap [classmap] → RuntimeException 'Could not scan for classes inside "<vendorDir>/dep/a/classmap" which does not appear to be a file nor a folder'; files [./test.php] → $vendorDir/dep/a/test.php
- suffix `_19`, dev=true

### testEmptyPaths
- root psr-0 {Foo→''}, psr-4 {Acme\Foo\→''}, classmap ['']; files Foo/Bar.php, class.php (Classmap\Foo)
- dump: dev=true, suffix `_15`
- asserts (inline): Foo→`$baseDir . '/'`; Acme\Foo\→`$baseDir . '/'`; classmap Classmap\Foo, Composer\InstalledVersions, Foo\Bar

### testVendorSubstringPath
- root psr-0 {Foo→composer-test-autoload-src/src}, psr-4 {Acme\Foo\→composer-test-autoload-src/src-psr4} (path shares a prefix with vendorDir)
- dump: dev=false, suffix `VendorSubstring`; asserts: Foo→$baseDir/composer-test-autoload-src/src; Acme\Foo\→$baseDir/composer-test-autoload-src/src-psr4

### testExcludeFromClassmap
- root psr-0 {Main→src/, Lala→[src/, lib/]}, psr-4 {Acme\Fruit\→src-fruit/, Acme\Cake\→[src-cake/, lib-cake/]}, classmap [composersrc/], exclude-from-classmap [/composersrc/foo/bar/, /composersrc/excludedTests/, /composersrc/ClassToExclude.php, /composersrc/*/excluded/excsubpath, **/excsubpath, composers, /src-ca/]
- files: src/Lala/ClassMapMain.php, src/Lala/Test/ClassMapMainTest.php, src-cake/ClassMapBar.php, composersrc/foo.php, composersrc/excludedTests/bar.php, composersrc/ClassToExclude.php, composersrc/long/excluded/excsubpath/foo.php, composersrc/long/excluded/excsubpath/bar.php, forks/bar/src/exclude/FooExclClass.php, symlink composersrc/foo/bar → forks/bar/
- dump: dev=true, suffix `_1`; asserts: autoload_classmap → classmap

### testGeneratesPlatformCheck (root `root/a` 1.0, dump dev=true, suffix `_1`)
| requires | provides | replaces | ignore-platform-reqs | expected `platform/<x>.php` |
|---|---|---|---|---|
| php ^7.2, ext-xml *, ext-json * | | | false | typical |
| php <8 | | | false | none |
| php >=7.2 | | | false | no_php_upper_bound |
| php ^7.2.8 | | | false | specific_php_release |
| php-64bit ^7.2.8 | | | false | specific_php_64bit_required |
| php-64bit * | | | false | php_64bit_required |
| ext-xml *, ext-json * | | | false | no_php_required |
| php ^7.2, ext-xml *, ext-json * | | | true | none |
| php ^7.2.8, ext-xml *, ext-json *, ext-pdo * | | | [php, ext-pdo] | no_php_required |
| php ^7.2.8, ext-xml, ext-json, ext-fileinfo, ext-filesystem, ext-filter (all *) | | | [php, ext-fil*] | no_php_required |
| php ^7.2 | | | false | no_extensions_required |
| ext-xml ^7.2, ext-pdo ^7.2, ext-bcmath ^7.2 | ext-xml *, ext-pdo 7.1.* | ext-pdo ^7.1, ext-bcmath ^7.2 | false | replaced_provided_exts |
"none" means no platform_check.php and autoload_real.php lacks the require line; otherwise the require line is present.

### testAbsoluteSymlinkWithPsr4DoesNotGenerateWarnings / WithClassmapExcludeFromClassmap
- root `test/package`; psr-4 {MyTools\→tools/} or classmap [tools/]; exclude-from-classmap [**/vendor/]; symlink tools → tools-real containing vendor/phpunit/... and MyClass.php
- asserts: no psr-4 warning; classmap has MyClass but not PHPUnit\Framework\Exception
