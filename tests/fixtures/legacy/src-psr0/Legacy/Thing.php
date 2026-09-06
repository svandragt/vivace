<?php
namespace Legacy;

// PSR-0 root package entry: exercises the namespace map alongside vendor
// packages' PSR-0 entries (pear/console_getopt, ezyang/htmlpurifier).
final class Thing
{
    public function name(): string
    {
        return 'legacy-thing';
    }
}
