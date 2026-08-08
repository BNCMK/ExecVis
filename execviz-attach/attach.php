<?php
// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: attach.php
//  script_path: execviz-attach/attach.php
//  module_name: attach
//  version: 0.53.1
//  description: Attaches the PHP adapter with no change to the program.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: attach, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

/**
 * Attaches the PHP adapter with no change to the program.
 *
 *   php -d auto_prepend_file=/path/to/execviz-attach/attach.php app.php
 *
 * or the same line in php.ini, which attaches every request on the host. The
 * program is not modified and includes nothing.
 */
if (getenv('EXECVIZ_COLLECTOR')) {
    try {
        require_once __DIR__ . '/../execviz-php/execviz.php';
        Execviz::install(getenv('EXECVIZ_COLLECTOR'),
                         getenv('EXECVIZ_HOST') ?: 'php',
                         getenv('EXECVIZ_DOMAIN') ?: 'app');

        // A process (or a request) is a unit of execution: without a span for
        // the run itself every captured line is dropped for having no parent.
        // The program really did run, so this is a true parent, not an invented
        // one.
        // Under PHP-FPM and mod_php one process serves one request, so the
        // span for the run IS the span for the request. It is named and
        // classified accordingly rather than left as a bare process, and the
        // response code decides its status, so a 500 is a failed span rather
        // than a successful one that happened to return an error.
        $isWeb  = isset($_SERVER['REQUEST_METHOD']);
        $method = $_SERVER['REQUEST_METHOD'] ?? '';
        $path   = strtok($_SERVER['REQUEST_URI'] ?? '', '?') ?: '';
        $name   = $isWeb
            ? trim($method . ' ' . $path)
            : basename($_SERVER['SCRIPT_NAME'] ?? 'php');
        $attrs  = ['pid' => getmypid()];
        if ($isWeb) {
            $attrs['method'] = $method;
            $attrs['path']   = substr($path, 0, 200);
        }
        $root = Execviz::i()->spanStart(substr($name, 0, 120), $isWeb ? 'io' : 'call',
            attributes: $attrs);
        Execviz::i()->makeCurrent($root);
        register_shutdown_function(static function () use ($root, $isWeb) {
            try {
                $code = $isWeb ? http_response_code() : 0;
                Execviz::i()->spanEnd($root, ($code >= 500) ? 'error' : 'ok',
                    attributes: $code ? ['status_code' => $code] : []);
                Execviz::i()->flush();
            } catch (Throwable $e) {
                // leaving anyway
            }
        });
        if (getenv('EXECVIZ_VERBOSE') === '1') {
            fwrite(STDERR, "execviz: attached to this process, no source change\n");
        }
    } catch (Throwable $e) {
        // a program must never fail to start because a recorder could not attach
        fwrite(STDERR, 'execviz: could not attach (' . $e->getMessage() . "); the program runs unchanged\n");
    }
}
