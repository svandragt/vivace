<?php

namespace Psr\Log;

/**
 * Describes log levels.
 */
class LogLevel
{
    // patched by cweagans/composer-patches fixture (root extra.patches)
    const EMERGENCY = 'emergency';
    const ALERT     = 'alert';
    const CRITICAL  = 'critical';
    const ERROR     = 'error';
    const WARNING   = 'warning';
    const NOTICE    = 'notice';
    const INFO      = 'info';
    const DEBUG     = 'debug';
}
