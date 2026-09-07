# Targets shell through devbox so php, composer, hyperfine, cargo-nextest
# and cargo-deny resolve.

.PHONY: install install-shim build test check bench bench-check profile hooks fixtures fmt record-packagist record-satis compat compat-refresh dist fuzz coverage

install:
	cargo install --path . --locked --bin viv

# Puts a `composer` binary on PATH that shadows the real Composer.
install-shim:
	cargo install --path . --locked --bin composer

build:
	devbox run -- cargo build --release

# Builds the release tarball for the host target, same layout as release.yml,
# for testing the release packaging locally.
dist: build
	$(eval TARGET := $(shell rustc -vV | sed -n 's/^host: //p'))
	$(eval TAG := $(shell git describe --tags --always))
	$(eval NAME := vivace-$(TAG)-$(TARGET))
	rm -rf "dist/$(NAME)"
	mkdir -p "dist/$(NAME)"
	cp target/release/viv target/release/composer LICENSE README.md "dist/$(NAME)/"
	tar czf "dist/$(NAME).tar.gz" -C dist "$(NAME)"
	@echo "dist/$(NAME).tar.gz"

test:
	devbox run -- cargo nextest run

check:
	devbox run -- cargo fmt --check
	devbox run -- cargo clippy --all-targets -- -D warnings
	devbox run -- cargo nextest run
	devbox run -- cargo deny check
	devbox run -- cargo machete
	devbox run -- env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

bench:
	devbox run -- bench/run.sh bench/laravel composer riff viv

bench-check:
	devbox run -- bench/run.sh tests/fixtures/monolog composer viv
	python3 bench/compare.py bench/results/viv.json --baseline bench/results/baseline.json

# Flamegraphs for #54/#55 (bench/results/profile.md). Needs `perf` access
# (`perf_event_paranoid <= 2` or `CAP_PERFMON`); errors with a message
# pointing at that setting otherwise, which is what happened in the sandbox
# this profiling first ran in — see profile.md's own note on the gap.
profile:
	devbox run -- cargo build --release
	devbox run -- bench/profile/install.sh bench/laravel
	devbox run -- bench/profile/install.sh bench/laravel -o
	devbox run -- bench/profile/update.sh bench/laravel

hooks:
	printf '#!/bin/sh\nexec make check\n' > .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit

fixtures:
	devbox run -- composer -d tests/fixtures/monolog install
	cp tests/fixtures/monolog/vendor/autoload.php tests/fixtures/monolog/expected/dev/
	cp tests/fixtures/monolog/vendor/composer/*.php tests/fixtures/monolog/vendor/composer/installed.json tests/fixtures/monolog/vendor/composer/LICENSE tests/fixtures/monolog/expected/dev/composer/
	devbox run -- composer -d tests/fixtures/monolog install --no-dev
	cp tests/fixtures/monolog/vendor/autoload.php tests/fixtures/monolog/expected/no-dev/
	cp tests/fixtures/monolog/vendor/composer/*.php tests/fixtures/monolog/vendor/composer/installed.json tests/fixtures/monolog/vendor/composer/LICENSE tests/fixtures/monolog/expected/no-dev/composer/
	devbox run -- composer -d tests/fixtures/legacy install
	cp tests/fixtures/legacy/vendor/autoload.php tests/fixtures/legacy/expected/dev/
	cp tests/fixtures/legacy/vendor/composer/*.php tests/fixtures/legacy/vendor/composer/installed.json tests/fixtures/legacy/vendor/composer/LICENSE tests/fixtures/legacy/expected/dev/composer/
	rm -rf tests/fixtures/legacy/expected/dev/bin && mkdir -p tests/fixtures/legacy/expected/dev/bin
	cp tests/fixtures/legacy/vendor/bin/* tests/fixtures/legacy/expected/dev/bin/
	devbox run -- composer -d tests/fixtures/legacy install --no-dev
	cp tests/fixtures/legacy/vendor/autoload.php tests/fixtures/legacy/expected/no-dev/
	cp tests/fixtures/legacy/vendor/composer/*.php tests/fixtures/legacy/vendor/composer/installed.json tests/fixtures/legacy/vendor/composer/LICENSE tests/fixtures/legacy/expected/no-dev/composer/

fmt:
	devbox run -- cargo fmt

record-packagist:
	./tests/fixtures/packagist/record.sh

record-satis:
	devbox run -- ./tests/fixtures/satis/record.sh

compat:
	devbox run -- cargo build --release
	devbox run -- compat/run.sh

compat-refresh:
	devbox run -- compat/refresh.sh

# `cargo fuzz` needs a nightly toolchain (sanitizer flags stable Rust
# doesn't accept): `rustup toolchain install nightly` once, devbox doesn't
# carry one. 30s per target, matching what CI runs before the nightly job's
# longer 120s: enough to prove a target still builds and finds nothing new.
fuzz:
	cd fuzz && for target in classmap_find_classes store_extract_archive lock_content_hash \
			repository_expand_minified require_manipulator semver_parse_constraint; do \
		echo "==> $$target"; \
		devbox run -- cargo +nightly fuzz run "$$target" -- -max_total_time=30 \
			"corpus/$$target" "seeds/$$target" || exit 1; \
	done

# Report only, no threshold: `docs/composer-contract.md`'s pipeline modules
# (the solver port, the lock writer) are what this is meant to surface gaps
# in, not a number to chase.
coverage:
	mkdir -p target/coverage
	devbox run -- cargo llvm-cov nextest --no-fail-fast --lcov --output-path target/coverage/lcov.info
	devbox run -- cargo llvm-cov report --summary-only
