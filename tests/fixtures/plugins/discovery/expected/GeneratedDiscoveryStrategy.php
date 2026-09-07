<?php

namespace Http\Discovery\Strategy;

class GeneratedDiscoveryStrategy implements DiscoveryStrategy
{
    public static function getCandidates($type)
    {
        switch ($type) {
            case 'Psr\\Http\\Message\\RequestFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];
            case 'Psr\\Http\\Message\\ResponseFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];
            case 'Psr\\Http\\Message\\ServerRequestFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];
            case 'Psr\\Http\\Message\\StreamFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];
            case 'Psr\\Http\\Message\\UploadedFileFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];
            case 'Psr\\Http\\Message\\UriFactoryInterface': return [['class' => 'Nyholm\\Psr7\\Factory\\Psr17Factory']];

            default: return [];
        }
    }
}
