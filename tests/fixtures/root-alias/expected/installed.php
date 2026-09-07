<?php return array(
    'root' => array(
        'name' => 'vivace/fixture-root-alias',
        'pretty_version' => '1.0.0+no-version-set',
        'version' => '1.0.0.0',
        'reference' => null,
        'type' => 'project',
        'install_path' => __DIR__ . '/../../',
        'aliases' => array(),
        'dev' => true,
    ),
    'versions' => array(
        'vivace/fixture-root-alias' => array(
            'pretty_version' => '1.0.0+no-version-set',
            'version' => '1.0.0.0',
            'reference' => null,
            'type' => 'project',
            'install_path' => __DIR__ . '/../../',
            'aliases' => array(),
            'dev_requirement' => false,
        ),
        'xwp/shortcode-ui-richtext' => array(
            'pretty_version' => 'dev-master',
            'version' => 'dev-master',
            'reference' => '4a060a3922cf989332d3586612d60578405992a0',
            'type' => 'wordpress-plugin',
            'install_path' => __DIR__ . '/../xwp/shortcode-ui-richtext',
            'aliases' => array(
                0 => '1.0.0',
                1 => '9999999-dev',
            ),
            'dev_requirement' => false,
        ),
    ),
);
