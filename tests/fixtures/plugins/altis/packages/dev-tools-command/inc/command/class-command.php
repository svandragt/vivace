<?php

namespace Altis\Dev_Tools\Command;

use Composer\Command\BaseCommand;
use DOMDocument;
use Exception;
use Symfony\Component\Console\Exception\CommandNotFoundException;
use Symfony\Component\Console\Exception\InvalidArgumentException;
use Symfony\Component\Console\Input\ArrayInput;
use Symfony\Component\Console\Input\InputArgument;
use Symfony\Component\Console\Input\InputInterface;
use Symfony\Component\Console\Input\InputOption;
use Symfony\Component\Console\Output\OutputInterface;
use Symfony\Component\Yaml\Yaml;

/**
 * Chassis command for Composer.
 */
class Command extends BaseCommand {
	/**
	 * Configure the command.
	 */
	protected function configure() {
		$this->setName( 'dev-tools' );
		$this->setDescription( 'Developer tools' );
		$this->setDefinition( [
			new InputArgument( 'subcommand', InputArgument::REQUIRED, 'phpunit | codecept | bootstrap | lintdocs' ),
			new InputOption( 'chassis', null, null, 'Run commands in the Local Chassis environment' ),
			new InputOption( 'path', 'p', InputArgument::OPTIONAL, 'Use a custom path for tests folder.', 'tests' ),
			new InputOption( 'output', 'o', InputArgument::OPTIONAL, 'Use a custom path for output folder.', '' ),
			new InputOption( 'browser', 'b', InputArgument::OPTIONAL, 'Run a headless Chrome browser for acceptance tests, use "chrome", or "firefox"', 'chrome' ),
			new InputOption( 'all', 'a', InputOption::VALUE_NONE, 'Run all suites even if one fails.' ),
			new InputOption( 'lint-path', 'l', InputArgument::OPTIONAL, 'Path to module to be linted.' ),
			new InputArgument( 'options', InputArgument::IS_ARRAY ),
		] );
		$this->setHelp(
			<<<EOT
Run a dev tools feature.

To run PHPUnit integration tests:
    phpunit [--chassis] [--] [options]
                                use `--` to separate arguments you want to
                                pass to phpunit. Use the --chassis option
                                if you are running Local Chassis.

To run Codeception commands:
    codecept [<subcommand>] [-p <path>] [-b <browser>] [-o <output-folder>] [-a] [<suite-name>] [--] [options]
                                Use -p to specify the path to tests folder.
                                Use -b to select a browser, eg: `chrome` or `firefox`.
                                Use -o to specify the output folder.
                                Use -a to continue executing all suites even if one fails.
                                Use `--` to send arguments to Codeception.

To Bootstrap configuration files:
    bootstrap <config> [--] [options]
                                <config> is the type of config to bootstrap, eg: codespaces | lintdocs

To lint the Markdown documentation files:
	lintdocs --lint-path|-l <path-to-module> files|markdown|style|all
		Use --lint-path to specify the relative path to the module to lint. The command will look for *.md files in 'docs', 'user-docs', and 'other-docs' sub-folders
		Use 'files' to check for the required files.
		Use 'markdown' to run markdown lint on the format of your markdown files.
		Use 'style' to run the Vale writing style analyser and spellcheck.
		Use 'all' to run all the documentation checks.

EOT
		);
	}

	/**
	 * Wrapper command to dispatch subcommands
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int Status code to return
	 */
	protected function execute( InputInterface $input, OutputInterface $output ) {
		$subcommand = $input->getArgument( 'subcommand' );
		switch ( $subcommand ) {
			case 'phpunit':
				return $this->phpunit( $input, $output );
			case 'codecept':
				return $this->codecept( $input, $output );
			case 'bootstrap':
				return $this->bootstrap( $input, $output );
			case 'lintdocs':
				return $this->lintdocs( $input, $output );

			default:
				throw new CommandNotFoundException( sprintf( 'Subcommand "%s" is not defined.', $subcommand ) );
		}
	}

	/**
	 * Runs PHPUnit with zero config by default.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function phpunit( InputInterface $input, OutputInterface $output ) {
		$options = [];

		// Get dev-tools config.
		$config = $this->get_config()['phpunit'] ?? [];

		// Set default directories and files.
		$test_paths = [ 'tests' ];

		// Get directories and files from config.
		if ( isset( $config['directories'] ) ) {
			$test_paths = array_merge( (array) $config['directories'], $test_paths );
		}

		$test_paths = array_map( function ( $path ) {
			return trim( $path, DIRECTORY_SEPARATOR );
		}, $test_paths );
		$test_paths = array_filter( $test_paths, [ $this, 'is_valid_test_path' ] );
		$test_paths = array_unique( $test_paths );

		// Check last option for a specific file path and override config if so.
		$options = $input->getArgument( 'options' );
		$maybe_test_path = $options[ count( $options ) - 1 ] ?? false;
		if ( $this->is_valid_test_path( $maybe_test_path ) ) {
			array_pop( $options );
			$test_paths = [ $maybe_test_path ];
		}

		// Get excludes from config.
		$excludes = (array) ( $config['excludes'] ?? [] );
		$excludes = array_map( function ( $path ) {
			return trim( $path, DIRECTORY_SEPARATOR );
		}, $excludes );
		$excludes = array_filter( $excludes, [ $this, 'is_valid_test_path' ] );
		$excludes = array_unique( $excludes );

		// Write XML config.
		$doc = new DOMDocument( '1.0', 'utf-8' );

		// Create PHPUnit Element.
		$phpunit = $doc->createElement( 'phpunit' );
		$phpunit->setAttribute( 'bootstrap', 'altis/dev-tools/inc/phpunit/bootstrap.php' );
		$phpunit->setAttribute( 'backupGlobals', 'false' );
		$phpunit->setAttribute( 'colors', 'true' );
		$phpunit->setAttribute( 'convertErrorsToExceptions', 'true' );
		$phpunit->setAttribute( 'convertNoticesToExceptions', 'true' );
		$phpunit->setAttribute( 'convertWarningsToExceptions', 'true' );

		// Allow overrides and additional attributes.
		if ( isset( $config['attributes'] ) ) {
			foreach ( $config['attributes'] as $name => $value ) {
				$phpunit->setAttribute( $name, $value );
			}
		}

		// Create testsuites.
		$testsuites = $doc->createElement( 'testsuites' );

		// Create testsuite.
		$testsuite = $doc->createElement( 'testsuite' );
		$testsuite->setAttribute( 'name', 'project' );

		foreach ( $test_paths as $test_path ) {
			if ( is_file( $this->get_root_dir() . DIRECTORY_SEPARATOR . $test_path ) ) {
				$tag = $doc->createElement( 'file', "../{$test_path}/" );
				$testsuite->appendChild( $tag );
			} else {
				$tag = $doc->createElement( 'directory', "../{$test_path}/" );
				// class-test-*.php
				$variant = $tag->cloneNode( true );
				$variant->setAttribute( 'prefix', 'class-test-' );
				$variant->setAttribute( 'suffix', '.php' );
				$testsuite->appendChild( $variant );
				// test-*.php
				$variant = $tag->cloneNode( true );
				$variant->setAttribute( 'prefix', 'test-' );
				$variant->setAttribute( 'suffix', '.php' );
				$testsuite->appendChild( $variant );
				// *-test.php
				$variant = $tag->cloneNode( true );
				$variant->setAttribute( 'suffix', '-test.php' );
				$testsuite->appendChild( $variant );
			}
		}

		foreach ( $excludes as $exclude ) {
			$tag = $doc->createElement( 'exclude', "../{$exclude}/" );
			$testsuite->appendChild( $tag );
		}

		// Build the doc.
		$doc->appendChild( $phpunit );
		$phpunit->appendChild( $testsuites );
		$testsuites->appendChild( $testsuite );

		// Add extensions if set.
		if ( isset( $config['extensions'] ) ) {
			$extensions = $doc->createElement( 'extensions' );
			foreach ( (array) $config['extensions'] as $class ) {
				$extension = $doc->createElement( 'extension' );
				$extension->setAttribute( 'class', $class );
				$extensions->appendChild( $extension );
			}
			$phpunit->appendChild( $extensions );
		}

		// Write the file.
		// phpcs:ignore WordPress.WP.AlternativeFunctions.file_system_read_file_put_contents
		file_put_contents(
			$this->get_root_dir() . DIRECTORY_SEPARATOR . 'vendor' . DIRECTORY_SEPARATOR . 'phpunit.xml',
			$doc->saveXML()
		);

		// Check for passed config option.
		if ( ! preg_match( '/(-c|--configuration)\s+/', implode( ' ', $options ) ) ) {
			$options = array_merge(
				[ '-c', 'vendor/phpunit.xml' ],
				$options
			);
		}

		return $this->run_command( $input, $output, 'vendor/bin/phpunit', $options );
	}

	/**
	 * Runs Codecept with zero config by default.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function codecept( InputInterface $input, OutputInterface $output ) {
		$project_subdomain = $this->get_project_subdomain();
		$tests_folder = rtrim( $input->getOption( 'path' ), '\\/' );
		$output_folder = $input->getOption( 'output' );
		$run_headless_browser = $input->getOption( 'browser' );
		$use_chassis = $input->getOption( 'chassis' );
		$options = $input->getArgument( 'options' );
		$subsubcommand = $options[0] ?? null;

		// Working directory for codeception is `vendor`, so need to go up once to resolve relative paths correctly.
		$tests_folder = '../' . $tests_folder;

		$folders = [
			'_data' => 'altis/dev-tools/tests/_data',
			'_support' => 'altis/dev-tools/tests/_support',
			'_envs' => 'altis/dev-tools/tests/_env',
			'_output' => "{$tests_folder}/_output",
		];

		foreach ( $folders as $folder => $_ ) {
			if ( file_exists( $tests_folder . '/' . $folder ) ) {
				$folders[ $folder ] = $tests_folder . '/' . $folder;
			}
		}

		// Allow custom output folder.
		if ( $output_folder ) {
			$folders['_output'] = $output_folder;
		}

		// Write the default config.
		$config = [
			'paths' => [
				'tests' => $tests_folder,
				'output' => $folders['_output'],
				'data' => $folders['_data'],
				'support' => $folders['_support'],
				'envs' => $folders['_envs'],
			],
			'actor_suffix' => 'Tester',
			'extensions' => [
				'enabled' => [
					'Codeception\Extension\RunFailed',
				],
				'commands' => [
					'lucatume\WPBrowser\Command\GenerateWPUnit',
					'lucatume\WPBrowser\Command\GenerateWPRestApi',
					'lucatume\WPBrowser\Command\GenerateWPRestController',
					'lucatume\WPBrowser\Command\GenerateWPRestPostTypeController',
					'lucatume\WPBrowser\Command\GenerateWPAjax',
					'lucatume\WPBrowser\Command\GenerateWPCanonical',
					'lucatume\WPBrowser\Command\GenerateWPXML',
				],
			],
			'modules' => [
				'config' => [
					'WPDb' => [
						'dsn' => '%TEST_SITE_DB_DSN%',
						'user' => '%TEST_SITE_DB_USER%',
						'password' => '%TEST_SITE_DB_PASSWORD%',
						'dump' => '%TEST_SITE_DB_DUMP%',
						'populator' => sprintf(
							'export DB_NAME=%1$s && ' .
							'wp core multisite-install --quiet --url=%2$s --base=/ --title=Testing ' .
							'--admin_user=admin --admin_password=password --admin_email=admin@%3$s ' .
							'--skip-email --skip-config && wp altis migrate --url=%2$s',
							'%TEST_SITE_DB_NAME%',
							'%TEST_SITE_WP_URL%',
							'%TEST_SITE_WP_DOMAIN%'
						),
						'populate' => true,
						'cleanup' => false,
						'waitlock' => 10,
						'url' => '%TEST_SITE_WP_URL%',
						'urlReplacement' => false,
						'tablePrefix' => '%TEST_SITE_TABLE_PREFIX%',
						'letAdminEmailVerification' => true,
						'letCron' => true,
					],
					'WPCLI' => [
						'path' => '/usr/src/app/wordpress',
						'url' => '%TEST_SITE_WP_DOMAIN%',
					],
					'WPBrowser' => [
						'url' => '%TEST_SITE_WP_URL%',
						'adminUsername' => '%TEST_SITE_ADMIN_USERNAME%',
						'adminPassword' => '%TEST_SITE_ADMIN_PASSWORD%',
						'adminPath' => '%TEST_SITE_WP_ADMIN_PATH%',
						'headers' => [
							'X_TEST_REQUEST' => 1,
							'X_WPBROWSER_REQUEST' => 1,
						],
					],
					'WPWebDriver' => [
						'url' => '%TEST_SITE_WP_URL%',
						'adminUsername' => '%TEST_SITE_ADMIN_USERNAME%',
						'adminPassword' => '%TEST_SITE_ADMIN_PASSWORD%',
						'adminPath' => '%TEST_SITE_WP_ADMIN_PATH%',
						'browser' => $run_headless_browser,
						'host' => '172.17.0.1',
						'port' => '4444',
						'wait' => 20,
						'window_size' => false, // disabled for Chrome driver.
						'capabilities' => [
							'acceptInsecureCerts' => true,
							'unexpectedAlertBehaviour' => 'accept',
							'goog:chromeOptions' => [
								'args' => [
									'--headless',
									'--disable-gpu',
									'--proxy-server=\'direct://\'',
									'--proxy-bypass-list=*',
									'--user-agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_14_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/95.0.4638.69 Safari/537.36 wp-browser"',
								],
							],
							'moz:firefoxOptions' => [
								'args' => [
									'-headless',
								],
								'prefs' => [
									'general.useragent.override' => 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.14; rv:70.0) Gecko/20100101 Firefox/70.0 wp-browser',
								],
							],
							'EdgeOptions' => [
								'args' => [
									'-user-agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Chrome/95.0.4638.69 Safari/604.1 Edg/95.0.100.0 wp-browser"',
								],
							],
						],
					],
					'WPFilesystem' => [
						'wpRootFolder' => '%WP_ROOT_FOLDER%',
						'plugins' => '.%WP_CONTENT_FOLDER%/plugins',
						'mu-plugins' => '%WP_CONTENT_FOLDER%/mu-plugins',
						'themes' => '%WP_CONTENT_FOLDER%/themes',
						'uploads' => '%WP_CONTENT_FOLDER%/uploads',
					],
					'WPLoader' => [
						'wpRootFolder' => '%WP_ROOT_FOLDER%',
						'dbName' => '%TEST_DB_NAME%',
						'dbHost' => '%TEST_DB_HOST%',
						'dbUser' => '%TEST_DB_USER%',
						'dbPassword' => '%TEST_DB_PASSWORD%',
						'dbCharset' => 'utf8mb4',
						'tablePrefix' => '%TEST_TABLE_PREFIX%',
						'domain' => '%TEST_SITE_WP_DOMAIN%',
						'adminEmail' => '%TEST_SITE_ADMIN_EMAIL%',
						'title' => 'Test',
						'theme' => '',
						'plugins' => [],
						'activatePlugins' => [],
						'multisite' => true,
						'configFile' => 'vendor/altis/dev-tools/inc/codeception/config.php',
						'contentFolder' => 'content',
						'bootstrapActions' => [
							'bootstrap_codeception_wp',
						],
					],
				],
			],
			'params' => [
				'codeception.env',
			],
			'settings' => [
				'error_level' => 'E_ALL & ~E_STRICT & ~E_DEPRECATED & ~E_USER_WARNING',
			],
		];

		// Merge config from composer.json.
		$overrides = $this->get_config()['codeception'] ?? [];
		$config = $this->merge_config( $config, $overrides );

		// Convert to YAML.
		$yaml = Yaml::dump( $config, 2, 4, Yaml::DUMP_MULTI_LINE_LITERAL_BLOCK );

		$db_host = $use_chassis ? 'localhost' : 'db';
		$test_env = <<<EOL
TEST_SITE_DB_DSN=mysql:host=$db_host;dbname=test
TEST_SITE_DB_HOST=$db_host
TEST_SITE_DB_NAME=test
TEST_SITE_DB_USER=wordpress
TEST_SITE_DB_PASSWORD=wordpress
TEST_SITE_DB_DUMP=altis/dev-tools/tests/_data/dump.sql
TEST_SITE_TABLE_PREFIX=wp_
TEST_SITE_ADMIN_USERNAME=admin
TEST_SITE_ADMIN_PASSWORD=password
TEST_SITE_WP_ADMIN_PATH=/wp-admin
TEST_SITE_WP_URL=https://$project_subdomain.altis.dev
TEST_SITE_WP_DOMAIN=$project_subdomain.altis.dev
TEST_SITE_ADMIN_EMAIL=admin@example.org
WP_ROOT_FOLDER=wordpress
WP_CONTENT_FOLDER=../content
TEST_DB_NAME=test2
TEST_DB_HOST=$db_host
TEST_DB_USER=wordpress
TEST_DB_PASSWORD=wordpress
TEST_TABLE_PREFIX=wp_
EOL;

		// Write the file.
		// phpcs:ignore WordPress.WP.AlternativeFunctions.file_system_read_file_put_contents
		file_put_contents(
			$this->get_root_dir() . DIRECTORY_SEPARATOR . 'vendor' . DIRECTORY_SEPARATOR . 'codeception.yml',
			$yaml
		);

		// Write the env file.
		// phpcs:ignore WordPress.WP.AlternativeFunctions.file_system_read_file_put_contents
		file_put_contents(
			$this->get_root_dir() . DIRECTORY_SEPARATOR . 'vendor' . DIRECTORY_SEPARATOR . 'codeception.env',
			$test_env
		);

		// Check for passed config option.
		if ( ! preg_match( '/(-c|--configuration)\s+/', implode( ' ', $options ) ) ) {
			$input->setArgument( 'options', array_merge(
				$options,
				in_array( '--', $options, true ) ? [] : [ '--' ],
				[ '-c', 'vendor/codeception.yml' ]
			) );
		}

		if ( $subsubcommand === 'bootstrap' ) {
			return $this->codecept_bootstrap( $input, $output );
		} elseif ( $subsubcommand === 'run' ) {
			$return = $this->codecept_run( $input, $output );
		} else {
			$return = $this->codecept_passthru( $input, $output );
		}

		return $return;
	}

	/**
	 * Bootstraps configuration files.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function bootstrap( InputInterface $input, OutputInterface $output ) : int {
		$configs = [ 'codespaces', 'lintdocs' ];

		$options = $input->getArgument( 'options' );
		$subsubcommand = $options[0] ?? null;

		if ( empty( $subsubcommand ) ) {
			$output->writeln( '<warning>You need to select a configuration set to bootstrap/generate.</warning>' );
			$output->writeln( sprintf( 'Available configuration sets are: %s.', implode( ', ', $configs ) ) );
			$output->writeln( 'eg: composer dev-tools bootstrap codespaces' );

			return 1;
		}

		if ( ! in_array( $subsubcommand, $configs, true ) ) {
			$output->writeln( sprintf( '<error>Could not find the target configuration set generator for "%s".</error>', $subsubcommand ) );
			$output->writeln( sprintf( 'Available configuration sets are: %s.', implode( ', ', $configs ) ) );

			return 1;
		}

		call_user_func( [ $this, 'bootstrap_' . $subsubcommand ], $input, $output );

		return 0;
	}

	/**
	 * Bootstraps Codespaces devcontainer configuration files.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function bootstrap_codespaces( InputInterface $input, OutputInterface $output ) : int {
		$target_folder = getcwd() . '/.devcontainer';
		$template_folder = realpath( __DIR__ . '/../../templates/.devcontainer' );

		if ( file_exists( $target_folder ) ) {
			$output->writeln( '<error>Codespaces devcontainer configuration exists already at <root>/.devcontainer, aborting.</error>' );

			return 1;
		}

		$base_command = sprintf( 'cp -r "%s" "%s" &> /dev/null', $template_folder, $target_folder );
		passthru( $base_command, $return_var );

		if ( $return_var ) {
			$output->writeln( '<error>Could not generate Codespaces devcontainer configuration, aborting.</error>' );

			return 1;
		}

		$output->writeln( '<info>Files have been copied to the root of your project in .devcontainer folder.</info>' );
		$output->writeln( '<info>Feel free to edit the generated config files to install system packages or editor extensions as needed.</info>' );
		$output->writeln( '<info>Once ready, visit https://github.com/codespaces/new to create a new Codespace for your project repo.</info>' );

		return 0;
	}

	/**
	 * Run the passed command on either the local-server or local-chassis environment.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 * @param string $command The command to run.
	 * @param array $options Any required options to pass to the command.
	 *
	 * @return int
	 */
	protected function run_command( InputInterface $input, OutputInterface $output, string $command, array $options = [] ) {
		$use_chassis = $input->getOption( 'chassis' );
		$cli = $this->getApplication()->find( $use_chassis ? 'chassis' : 'server' );

		// Add the command, default options and input options together.
		$options = array_merge(
			[ $command ],
			$options
		);

		$return_val = $cli->run( new ArrayInput( [
			'subcommand' => 'exec',
			'options' => $options,
		] ), $output );

		return $return_val;
	}

	/**
	 * Bootstrap tests folder.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function codecept_bootstrap( InputInterface $input, OutputInterface $output ) : int {
		$default_suites = $this->get_test_suites( 'vendor/altis/dev-tools/tests/_templates' );

		$tests_folder = rtrim( $input->getOption( 'path' ), '\\/' );
		$selected_suites = $input->getArgument( 'options' )[1] ?? '';

		// If no option argument is found, and we hit a parameter argument,
		// then no suite is specified. eg: codecept run -- -o something.
		if ( 0 === strpos( $selected_suites, '-' ) ) {
			$selected_suites = [];
		} else {
			$selected_suites = explode( ',', $selected_suites );
		}

		if ( empty( $selected_suites ) ) {
			$selected_suites = $default_suites;
		}

		if ( $selected_suites ) {
			$invalid = array_diff( $selected_suites, $default_suites );
			if ( count( $invalid ) ) {
				throw new Exception(
					sprintf(
						'Invalid suites selected: "%s", available suites are: "%s".',
						implode( ',', $invalid ),
						implode( ',', $default_suites )
					)
				);
			}
		}

		$base_path = getcwd(); // Do we have another way to detect this ?
		$tests_folder = $base_path . '/' . $tests_folder;

		if ( ! file_exists( $tests_folder ) ) {
			mkdir( $tests_folder, 0755, true );
		} else {
			foreach ( $selected_suites as $suite ) {
				if ( file_exists( $tests_folder . '/' . $suite ) ) {
					throw new Exception( sprintf( 'An existing "%s" suite was found, halting execution.', $suite ) );
				}
			}
		}

		$template_path = 'vendor/altis/dev-tools/tests/_templates/%s.suite.yml';
		foreach ( $selected_suites as $suite ) {
			$suite_path = sprintf( '%s/%s.suite.yml', $tests_folder, $suite );
			copy( sprintf( $template_path, $suite ), $suite_path );
			mkdir( $tests_folder . '/' . $suite, 0755, true );
		}

		$output->write( sprintf( '<info>Created test suites (%s) in %s.</info>', implode( ',', $selected_suites ), $tests_folder ) );

		return 0;
	}

	/**
	 * Run Codeception on selectde/all test suites.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function codecept_run( InputInterface $input, OutputInterface $output ) : int {
		$tests_folder = '../' . rtrim( $input->getOption( 'path' ), '\\/' );
		$options = $input->getArgument( 'options' );
		$test_suite = isset( $options[1] ) && 0 !== strpos( $options[1], '-' ) ? $options[1] : '';

		if ( $test_suite ) {
			$suites = explode( ',', $test_suite );
		} else {
			// Codeception command runs within `vendor` directory, so paths are relative to that.
			$suites = $this->get_test_suites( 'vendor/' . $tests_folder );
		}

		$output->write(
			sprintf( '<info>Running %d suite(s), (%s)..</info>', count( $suites ), implode( ', ', $suites ) ),
			true,
			$output::VERBOSITY_NORMAL
		);

		$return = 0;

		// Ensure cache is clean before adding `.test-running` file and entering test mode.
		$this->flush_cache( $input, $output );

		// Write temp file during test run.
		$temp_run_file_path = $this->get_root_dir() . DIRECTORY_SEPARATOR . 'vendor' . DIRECTORY_SEPARATOR . '.test-running';
		// phpcs:ignore WordPress.WP.AlternativeFunctions.file_system_read_file_put_contents
		file_put_contents( $temp_run_file_path, 'true' );
		register_shutdown_function( function () use ( $temp_run_file_path ) {
			unlink( $temp_run_file_path );
		} );

		// Track db and browser container creation.
		$has_created_db = false;
		$has_created_browser = false;

		// Iterate over the suites.
		foreach ( $suites as $suite ) {
			$output->write( sprintf( '<info>Running "%s" test suite..</info>', $suite ), true, $output::VERBOSITY_NORMAL );

			// Create database if needed.
			$using_wpdb = $this->suite_has_module( 'vendor/' . $tests_folder . '/' . $suite . '.suite.yml', 'WPDb' );
			$using_wploader = $this->suite_has_module( 'vendor/' . $tests_folder . '/' . $suite . '.suite.yml', 'WPLoader' );
			if ( ! $has_created_db && ( $using_wpdb || $using_wploader ) ) {
				$this->create_test_db( $input, $output );
				$has_created_db = true;

				// Remove the db on shutdown.
				register_shutdown_function( function () use ( $input, $output ) {
					$output->write( '<info>Removing test databases..</info>', true, $output::VERBOSITY_NORMAL );
					$this->delete_test_db( $input, $output );
				} );
			}

			// Run the headless browser container if needed.
			if ( ! $has_created_browser && $this->suite_has_module( 'vendor/' . $tests_folder . '/' . $suite . '.suite.yml', 'WPWebDriver' ) ) {
				// Start a new container.
				$this->start_browser_container( $input, $output );
				$has_created_browser = true;

				// Stop the container on shutdown.
				register_shutdown_function( function () use ( $input, $output ) {
					$output->write( '<info>Removing headless browser container..</info>', true, $output::VERBOSITY_NORMAL );
					$this->stop_browser_container( $input, $output );
				} );
			}

			// Add the suite name to options here, if not passed already, and we have more than one.
			$suite_options = $this->add_suite_name_to_options( $options, $suite, $test_suite );

			$output->write( '<info>Running Codeception..</info>', true, $output::VERBOSITY_NORMAL );
			$return = $return ?: $this->run_command( $input, $output, 'vendor/bin/codecept', $suite_options );

			// Ensure cache is clean before next suite.
			$this->flush_cache( $input, $output );

			// Stop executing the rest of the suites if one fails and `continue` argument is absent.
			if ( $return && ! $input->getOption( 'all' ) ) {
				break;
			}
		}

		return $return;
	}

	/**
	 * Run Codeception arbitrary commands.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function codecept_passthru( InputInterface $input, OutputInterface $output ) : int {
		$options = $input->getArgument( 'options' );
		$output->write( '<info>Running Codeception..</info>', true, $output::VERBOSITY_NORMAL );

		return $this->run_command( $input, $output, 'vendor/bin/codecept', $options );
	}

	/**
	 * Create test databases.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function create_test_db( InputInterface $input, OutputInterface $output ) {
		$use_chassis = $input->getOption( 'chassis' );

		$sql_query = 'DROP DATABASE IF EXISTS test; DROP DATABASE IF EXISTS test2; CREATE DATABASE test; CREATE DATABASE test2; GRANT ALL PRIVILEGES ON test.* TO wordpress; GRANT ALL PRIVILEGES ON test2.* TO wordpress;';

		if ( $use_chassis ) {
			$cli = $this->getApplication()->find( 'chassis' );

			$return_val = $cli->run( new ArrayInput( [
				'subcommand' => 'exec',
				'options' => [
					'mysql',
					'-uroot',
					'-ppassword',
					'-e',
					'"' . $sql_query . '"',
				],
			] ), $output );
		} else {
			$cli = $this->getApplication()->find( 'server' );

			$return_val = $cli->run( new ArrayInput( [
				'subcommand' => 'db',
				'options' => [
					'exec',
					$sql_query,
				],
			] ), $output );
		}

		return $return_val;
	}

	/**
	 * Create test databases.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function delete_test_db( InputInterface $input, OutputInterface $output ) {
		$use_chassis = $input->getOption( 'chassis' );

		$sql_query = 'DROP DATABASE test; DROP DATABASE test2; REVOKE ALL PRIVILEGES on test.* FROM wordpress; REVOKE ALL PRIVILEGES on test2.* FROM wordpress;';

		if ( $use_chassis ) {
			$cli = $this->getApplication()->find( 'chassis' );

			$return_val = $cli->run( new ArrayInput( [
				'subcommand' => 'exec',
				'options' => [
					'mysql',
					'-uroot',
					'-ppassword',
					'-e',
					'"' . $sql_query . '"',
				],
			] ), $output );
		} else {
			$cli = $this->getApplication()->find( 'server' );

			$return_val = $cli->run( new ArrayInput( [
				'subcommand' => 'db',
				'options' => [
					'exec',
					$sql_query,
				],
			] ), $output );
		}

		// Remove ES indexes.
		$return_val = $cli->run( new ArrayInput( [
			'subcommand' => 'exec',
			'options' => [
				'curl',
				'--silent',
				'-o',
				'/dev/null',
				'-XDELETE',
				'http://elasticsearch:9200/ep-tests-*',
			],
		] ), $output );

		return $return_val;
	}

	/**
	 * Get the name of the project for the local subdomain
	 *
	 * @return string
	 */
	protected function get_project_subdomain() : string {

		// phpcs:ignore WordPress.WP.AlternativeFunctions.file_get_contents_file_get_contents
		$composer_json = json_decode( file_get_contents( getcwd() . '/composer.json' ), true );

		if ( isset( $composer_json['extra']['altis']['modules']['local-server']['name'] ) ) {
			$project_name = $composer_json['extra']['altis']['modules']['local-server']['name'];
		} else {
			$project_name = basename( getcwd() );
		}

		return preg_replace( '/[^A-Za-z0-9\-\_]/', '', $project_name );
	}

	/**
	 * Clean the redis cache.
	 *
	 * We avoid relying on WP CLI because of the different databases / contexts used for
	 * different suites. It can result in database connection failed errors.
	 *
	 * @param
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function flush_cache( InputInterface $input, OutputInterface $output ) {
		$use_chassis = $input->getOption( 'chassis' );

		$output->writeln( 'Flushing the cache...' );

		if ( $use_chassis ) {
			$return_val = $this->run_command( $input, $output, 'redis-cli FLUSHALL' );
		} else {
			$base_command = sprintf(
				'docker exec %1$s-redis redis-cli FLUSHALL &> /dev/null',
				$this->get_project_subdomain()
			);
			passthru( $base_command, $return_val );
		}

		if ( $return_val ) {
			$output->writeln( '<error>Cache could not be flushed, the Redis service may not be in use or not running.</>' );
		}

		return $return_val;
	}

	/**
	 * Start a headless browser container for WebDriver used in acceptance tests.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function start_browser_container( InputInterface $input, OutputInterface $output ) {
		$columns = exec( 'tput cols' );
		$lines = exec( 'tput lines' );
		$browser = $input->getOption( 'browser' ) ?: 'chrome';
		$output->write( sprintf( '<info>Starting headless "%s" browser container..</info>', $browser ), true, $output::VERBOSITY_NORMAL );

		$available_browsers = [
			'chromium' => 'chromium',
			'chrome' => 'chromium',
			'firefox' => 'firefox',
			// 'edge' => 'edge', // TODO Buggy driver, dig deeper.
		];

		if ( ! array_key_exists( $browser, $available_browsers ) ) {
			throw new InvalidArgumentException( sprintf(
				'Browser "%s" is unavailable, available browsers are: %s.',
				$browser,
				implode( ', ', array_keys( $available_browsers ) ),
			) );
		}

		// Stop any lingering containers first.
		$this->stop_browser_container( $input, $output );

		// This exports ports 4444 for the Selenium hub web portal, and 7900 for the noVNC server.
		$base_command = sprintf(
			'docker run ' .
			'-d ' .
			'-e COLUMNS=%1%d -e LINES=%2$d ' .
			'--network=host ' .
			'--name=%3$s_selenium ' .
			'--shm-size="2g" ' .
			'seleniarm/standalone-%4$s:4.1.4-20220429',
			$columns,
			$lines,
			$this->get_project_subdomain(),
			$available_browsers[ $browser ],
		);

		passthru( $base_command, $return_var );

		// Allow time for selenium app to boot up.
		sleep( 5 );

		return $return_var;
	}

	/**
	 * Stop the headless browser container.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function stop_browser_container( InputInterface $input, OutputInterface $output ) {
		$base_command = sprintf( 'docker ps -q -a --filter "name=%1$s_selenium" | grep -q . && docker rm -f %1$s_selenium &> /dev/null', $this->get_project_subdomain() );

		passthru( $base_command, $return_var );

		return $return_var;
	}

	/**
	 * Return suite files within a tests folder.
	 *
	 * @param string $folder Tests folder to scan.
	 *
	 * @return array
	 */
	protected function get_test_suites( $folder ) : array {
		$suites = glob( $folder . '/*.suite.yml' );
		foreach ( $suites as $i => $suite ) {
			$suites[ $i ] = substr( $suite, strlen( $folder ) + 1, strpos( $suite, '.suite.yml' ) - strlen( $suite ) );
		}

		return $suites;
	}

	/**
	 * Returns whether a suite configuration requires a specific module.
	 *
	 * @param string $suite_file Suite YML file.
	 * @param string $module Module name.
	 *
	 * @return boolean
	 */
	protected function suite_has_module( string $suite_file, string $module ) : bool {
		$suite_config = file_get_contents( $suite_file );
		$needs_web_driver = preg_match( "#- {$module}#", $suite_config );

		return $needs_web_driver;
	}

	/**
	 * Add suite name to options passed to codecept command.
	 *
	 * @param array $options Options array.
	 * @param string $suite_name Suite name.
	 * @param string $test_suite_arg Test suite parameter from CLI args.
	 *
	 * @return array
	 */
	protected function add_suite_name_to_options( array $options, string $suite_name, string $test_suite_arg ) : array {
		if ( $suite_name === $test_suite_arg ) {
			return $options;
		}
		// If we had a test suite param from CLI args, remove it first.
		if ( $test_suite_arg ) {
			$options = array_diff( $options, [ $test_suite_arg ] );
		}

		// Then add the current test suite name.
		$index = array_search( 'run', $options, true );
		array_splice( $options, $index + 1, 0, [ $suite_name ] );

		return $options;
	}

	/**
	 * Get the root directory path for the project.
	 *
	 * @return string
	 */
	protected function get_root_dir() : string {
		return dirname( $this->getComposer()->getConfig()->getConfigSource()->getName() );
	}

	/**
	 * Get a module config from composer.json.
	 *
	 * @param string $module The module to get the config for.
	 *
	 * @return array
	 */
	protected function get_config( $module = 'dev-tools' ) : array {
		// @codingStandardsIgnoreLine
		$json = file_get_contents( $this->get_root_dir() . DIRECTORY_SEPARATOR . 'composer.json' );
		$composer_json = json_decode( $json, true );

		return (array) ( $composer_json['extra']['altis']['modules'][ $module ] ?? [] );
	}

	/**
	 * Check if a given path is valid for PHPUnit.
	 *
	 * @param string $path The filepath to check.
	 *
	 * @return boolean
	 */
	protected function is_valid_test_path( string $path ) : bool {
		if ( empty( $path ) ) {
			return false;
		}
		$full_path = $this->get_root_dir() . DIRECTORY_SEPARATOR . $path;
		if ( strpos( $path, '*' ) !== false ) {
			return ! empty( glob( $full_path ) );
		}
		if ( is_file( $full_path ) ) {
			return in_array( pathinfo( $full_path, PATHINFO_EXTENSION ), [ 'php', 'inc' ], true );
		}

		return is_dir( $full_path );
	}

	/**
	 * Merges two configuration arrays together, overriding the first or adding
	 * to it with items from the second.
	 *
	 * @param array $config The default config array.
	 * @param array $overrides The config to merge in.
	 *
	 * @return array
	 */
	protected function merge_config( array $config, array $overrides ) : array {
		$merged = $config;
		foreach ( $overrides as $key => $value ) {
			if ( is_string( $key ) ) {
				if ( is_array( $value ) ) {
					// Recursively merge arrays.
					$merged[ $key ] = $this->merge_config( $merged[ $key ] ?? [], $value );
				} else {
					// Overwrite scalar values directly.
					$merged[ $key ] = $value;
				}
			} else {
				// Merge numerically keyed arrays directly and remove empty/duplicate items.
				$merged = array_merge( $merged, (array) $overrides );
				$merged = array_filter( $merged );
				$merged = array_unique( $merged );
				break;
			}
		}

		return $merged;
	}

	/**
	 * Bootstraps Docs linter configuration files.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function bootstrap_lintdocs( InputInterface $input, OutputInterface $output ) : int {

		// Copy configuration file for Markdownlint
		$target_file = getcwd() . '/.markdownlint.yaml';
		$template_file = realpath( __DIR__ . '/../../templates/docslint/markdownlint.yaml' );

		if ( file_exists( $target_file ) ) {
			$output->writeln( '<error>Markdownlint configuration file already exists at <root>/.markdownlint.yml, not overwriting.</error>' );
		} else {

			$base_command = sprintf( 'cp "%s" "%s" &> /dev/null', $template_file, $target_file );
			passthru( $base_command, $return_var );

			if ( $return_var ) {
				$output->writeln( '<error>Could not create markdownlint configuration file, exiting.</error>' );

				return 1;
			}
		}

		// Copy styles folder for Vale
		$target_folder = getcwd() . '/styles';
		$template_folder = realpath( __DIR__ . '/../../templates/docslint/styles' );

		if ( file_exists( $target_folder ) ) {
			$output->writeln( '<error>Documentation linter Style folder already exists at <root>/styles, not overwriting.</error>' );
		} else {

			$base_command = sprintf( 'cp -r "%s" "%s" &> /dev/null', $template_folder, $target_folder );
			passthru( $base_command, $return_var );

			if ( $return_var ) {
				$output->writeln( '<error>Could not create documents linter styles folder, exiting.</error>' );

				return 1;
			}
		}

		// Copy configuration file for Vale
		$target_file = getcwd() . '/.vale.ini';
		$template_file = realpath( __DIR__ . '/../../templates/docslint/vale.ini' );

		if ( file_exists( $target_file ) ) {
			$output->writeln( '<error>Documentation linter configuration file already exists at <root>/.vale.ini, not overwriting.</error>' );
		} else {

			$base_command = sprintf( 'cp "%s" "%s" &> /dev/null', $template_file, $target_file );
			passthru( $base_command, $return_var );

			if ( $return_var ) {
				$output->writeln( '<error>Could not create documents linter configuration file, exiting.</error>' );

				return 1;
			}
		}

		$output->writeln( '<info>Files have been copied to the root of your project: .markdownlint.yml, styles/, and .vale.ini.</info>' );
		$output->writeln( '<info>Feel free to edit the generated config files to install Style guides or configure as needed.</info>' );

		return 0;
	}


	/**
	 * Runs Documentation Linting with default config by default.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function lintdocs( InputInterface $input, OutputInterface $output ) {
		$subcommands = [ 'files', 'markdown', 'style' ];
		$options = $input->getArgument( 'options' );
		$subsubcommand = $options[0] ?? null;

		$lint_path = $input->getOption( 'lint-path' ) ?? '.';
		$lint_path = rtrim( $lint_path, '\\/' );

		if ( ! in_array( $subsubcommand, $subcommands, true ) && ( $subsubcommand !== 'all' ) ) {
			$output->writeln( sprintf( 'Available sub commands are: %s.', implode( ', ', $subcommands ) ) );

			return 1;
		}

		// Check path exists
		if ( ! file_exists( $lint_path ) ) {
			$output->writeln( sprintf( '<error>Docslint cannot find the path you specified "%s", exiting.</error>', $lint_path ) );

			return 1;
		}

		if ( $subsubcommand === 'all' ) {
			foreach ( $subcommands as $subsubcommand ) {
				call_user_func( [ $this, 'lintdocs_' . $subsubcommand ], $input, $output );
			}
		} else {
			call_user_func( [ $this, 'lintdocs_' . $subsubcommand ], $input, $output );
		}

		return 0;
	}

	/**
	 * Runs the markdown files checker.
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function lintdocs_files( InputInterface $input, OutputInterface $output ) : int {
		$options = $input->getArgument( 'options' );
		$lint_path = rtrim( $input->getOption( 'lint-path' ), '\\/' );
		$options = $input->getArgument( 'options' );

		if ( empty( $lint_path ) ) {
			$output->writeln( sprintf( '<error>Folder not found %s, exiting.</error>', $lint_path ) );

			return 1;
		}

		$base_command = sprintf(
			'docker run ' .
			'--rm ' .
			'-v %s:/workdir ' .
			'node:16-alpine sh -c ' .
			'"node /workdir/vendor/altis/dev-tools-command/templates/docslint/hm-lint-markdown.js /workdir/%s" ',
			$this->get_root_dir(),
			$lint_path
		);

		$output->writeln( sprintf( 'executing: %s.', $base_command ) );

		$return_var = 0;

		passthru( $base_command, $return_var );

		return $return_var;
	}

	/**
	 * Runs the markdown linter .
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function lintdocs_markdown( InputInterface $input, OutputInterface $output ) : int {
		$options = $input->getArgument( 'options' );
		$lint_path = rtrim( $input->getOption( 'lint-path' ), '\\/' );
		$options = $input->getArgument( 'options' );

		// Manually add in the expected subdirectories.
		$docs_path_list = [];
		foreach ( $this->get_expected_docs_paths() as $docs_path ) {
			$target_path = getcwd() . '/' . $lint_path . $docs_path;
			if ( file_exists( $target_path ) ) {
				$docs_path_list[] = $lint_path . $docs_path . '/**/*.md';
			}
		}

		if ( empty( $docs_path_list ) ) {
			$output->writeln( sprintf( '<error>No docs folders found in supplied path %s, exiting.</error>', $lint_path ) );

			return 1;
		}

		$docs_path = implode( '" "', $docs_path_list );

		$base_command = sprintf(
			'docker run ' .
			'--rm ' .
			'-v %s:/workdir ' .
			'davidanson/markdownlint-cli2 ' .
			'"%s" ',
			$this->get_root_dir(),
			$docs_path
		);

		$output->writeln( sprintf( 'executing: %s.', $base_command ) );

		$return_var = 0;

		passthru( $base_command, $return_var );

		return $return_var;
	}

	/**
	 * Runs the Vale style checker linter .
	 *
	 * @param InputInterface $input
	 * @param OutputInterface $output
	 *
	 * @return int
	 */
	protected function lintdocs_style( InputInterface $input, OutputInterface $output ) : int {
		$options = $input->getArgument( 'options' );
		$lint_path = rtrim( $input->getOption( 'lint-path' ), '\\/' );
		$options = $input->getArgument( 'options' );

		// Need to check which docs folders exist as Vale does not like missing folders.
		$docs_path_list = [];
		foreach ( $this->get_expected_docs_paths() as $docs_path ) {
			$target_path = getcwd() . '/' . $lint_path . $docs_path;
			if ( file_exists( $target_path ) ) {
				$docs_path_list[] = $lint_path . $docs_path;
			}
		}

		if ( empty( $docs_path_list ) ) {
			$output->writeln( sprintf( '<error>No docs folders found in supplied path %s, exiting.</error>', $lint_path ) );

			return 1;
		}

		$docs_path = implode( ' ', $docs_path_list );
		$base_command = sprintf(
			'docker run ' .
			'--platform linux/amd64 ' .
			'--rm -v %s/styles:/styles ' .
			'--rm -v %s:/workdir ' .
			' -w /workdir ' .
			'jdkato/vale --config=/workdir/.vale.ini ' .
			'%s ',
			$this->get_root_dir(),
			$this->get_root_dir(),
			$docs_path
		);

		$output->writeln( sprintf( 'executing: %s.', $base_command ) );

		$return_var = 0;

		passthru( $base_command, $return_var );

		return $return_var;
	}

	/**
	 * Return a list of the document directories to lint.
	 *
	 * @return string[]
	 */
	protected function get_expected_docs_paths() : array {
		return [
			'/docs',
			'/user-docs',
			'/other-docs',
		];
	}

}
