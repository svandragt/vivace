<?php

$vendorDir = dirname(__DIR__);
$rootDir = dirname(dirname(__DIR__));

return array (
  'acme/craft-widget' => 
  array (
    'class' => 'acme\\craftwidget\\Plugin',
    'basePath' => './src',
    'handle' => 'craft-widget',
    'aliases' => 
    array (
      '@acme/craftwidget' => $vendorDir . '/acme/craft-widget/src',
    ),
    'name' => 'Widget',
    'version' => '1.0.0',
    'description' => 'An example Craft CMS plugin.',
    'developer' => 'Acme Corp',
    'developerUrl' => 'https://example.com/craft-widget',
    'developerEmail' => 'support@example.com',
    'documentationUrl' => 'https://example.com/craft-widget/docs',
  ),
);
