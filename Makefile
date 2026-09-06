# Targets shell through devbox so php, composer, hyperfine, cargo-nextest
# and cargo-deny resolve.

.PHONY: install build test check bench hooks fixtures fmt

install:
	cargo install --path . --locked

build:
	devbox run -- cargo build --release

test:
	devbox run -- cargo nextest run

check:
	devbox run -- cargo fmt --check
	devbox run -- cargo clippy --all-targets -- -D warnings
	devbox run -- cargo nextest run
	devbox run -- cargo deny check

bench:
	devbox run -- bench/run.sh bench/laravel composer riff viv

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

fmt:
	devbox run -- cargo fmt
