<?php

namespace Acme\Psr4;

class Greeter
{
    public function greet(string $name): string
    {
        return "Hello, $name";
    }
}
