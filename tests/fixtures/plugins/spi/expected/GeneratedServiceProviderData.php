<?php declare(strict_types=1);
namespace Nevay\SPI;

/**
 * @internal 
 */
final class GeneratedServiceProviderData {

    public const VERSION = 1;

    /**
     * @param class-string $service
     * @return list<class-string>
     */
    public static function providers(string $service): array {
        return match ($service) {
            default => [],
            \Acme\Spi\ServiceInterface::class => [
                \Acme\Spi\ConcreteProvider::class, // acme/spi-provider 1.0.0 (extra.spi)
            ],
        };
    }
}