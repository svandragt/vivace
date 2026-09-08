<?php

$vendorDir = dirname(__DIR__);

return array (
  'acme/yii2-widget' => 
  array (
    'name' => 'acme/yii2-widget',
    'version' => '1.0.0.0',
    'alias' => 
    array (
      '@Acme/Widget' => $vendorDir . '/acme/yii2-widget/src',
    ),
    'bootstrap' => 'Acme\\Widget\\Bootstrap',
  ),
);
