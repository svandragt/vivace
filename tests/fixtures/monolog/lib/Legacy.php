<?php
// Not PSR-4: lives in lib/ under a different namespace, found via classmap.
namespace Fixture\Legacy;

interface Marker {}

class LegacyThing implements Marker
{
    public const NAME = 'legacy';
}

enum Mode: string
{
    case On = 'on';
}
