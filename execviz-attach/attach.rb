# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: attach.rb
#  script_path: execviz-attach/attach.rb
#  module_name: attach
#  version: 0.53.1
#  description: Attaches the Ruby adapter with no change to the program.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: attach, adapter
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# Attaches the Ruby adapter with no change to the program.
#
#   RUBYOPT="-r/path/to/execviz-attach/attach" \
#   EXECVIZ_COLLECTOR=http://host:8900 ruby app.rb
#
# The program is not modified and requires nothing.
if ENV['EXECVIZ_COLLECTOR']
  begin
    require File.expand_path('../execviz-ruby/execviz', __dir__)
    Execviz.install(collector: ENV['EXECVIZ_COLLECTOR'],
                    host_id: ENV['EXECVIZ_HOST'] || 'ruby',
                    domain: ENV['EXECVIZ_DOMAIN'] || 'app')

    # A process is a unit of execution: without a span for the run itself every
    # captured line is dropped for having no parent. The program really did run.
    root = Execviz.span_start(File.basename($PROGRAM_NAME), 'call',
                              attributes: { 'argv' => ARGV.first(8).join(' ')[0, 400],
                                            'pid' => Process.pid })
    Fiber[:execviz_span] = root
    at_exit do
      begin
        Execviz.span_end(root, 'ok')
        Execviz.flush
      rescue StandardError
        nil
      end
    end
    # =========================================================================
    # REQUESTS BECOME SPANS
    # =========================================================================
    # A span for the process says a program ran and nothing about what it did.
    # Every request served and every request made is a unit of work with its own
    # timing and status, so each becomes a span. Without this the map holds one
    # span per process and `witness` has no claim to check against.
    begin
      require 'rack'
      # Rack is the entry point Rails, Sinatra and Hanami all pass through, so
      # wrapping the app catches a request whatever framework produced it.
      Rack::Builder.prepend(Module.new do
        def to_app
          Execviz::RackSpan.new(super)
        end
      end)
    rescue LoadError, StandardError
      nil                     # no Rack here, which is not a failure
    end

    begin
      require 'net/http'
      # Net::HTTP is what almost every Ruby client library ends in, so wrapping
      # `request` catches an outbound call without knowing who asked for it.
      Net::HTTP.prepend(Module.new do
        def request(req, body = nil, &block)
          target = "#{address}#{req.respond_to?(:path) ? req.path : ''}"
          span = Execviz.span_start("http out #{target}"[0, 120], 'external',
                                    attributes: { 'target' => target[0, 200] })
          begin
            resp = super
            code = resp.respond_to?(:code) ? resp.code.to_i : 0
            Execviz.span_end(span, code >= 500 ? 'error' : 'ok',
                             attributes: { 'status_code' => code })
            resp
          rescue StandardError
            Execviz.span_end(span, 'error')
            raise
          end
        end
      end)
    rescue LoadError, StandardError
      nil
    end

    warn 'execviz: attached to this process, no source change' if ENV['EXECVIZ_VERBOSE'] == '1'
  rescue StandardError => e
    # a program must never fail to start because a recorder could not attach
    warn "execviz: could not attach (#{e.message}); the program runs unchanged"
  end
end


# =========================================================================
# THE RACK MIDDLEWARE
# =========================================================================
module Execviz
  # One span per request served. The connection's descriptor is recorded where
  # the server exposes it, because that is what ties the read before the handler
  # and the write after it back to this request.
  class RackSpan
    def initialize(app)
      @app = app
    end

    def call(env)
      name = "#{env['REQUEST_METHOD']} #{env['PATH_INFO']}"[0, 120]
      attrs = { 'method' => env['REQUEST_METHOD'].to_s,
                'path' => env['PATH_INFO'].to_s[0, 200] }
      fd = descriptor_of(env)
      attrs['fd'] = fd if fd && fd >= 0
      span = Execviz.span_start(name, 'io', attributes: attrs)
      begin
        status, headers, body = @app.call(env)
        Execviz.span_end(span, status.to_i >= 500 ? 'error' : 'ok',
                         attributes: { 'status_code' => status.to_i })
        [status, headers, body]
      rescue StandardError
        Execviz.span_end(span, 'error')
        raise
      end
    end

    private

    def descriptor_of(env)
      sock = env['rack.hijack_io'] || env['puma.socket'] || env['rack.input']
      sock.respond_to?(:fileno) ? sock.fileno : -1
    rescue StandardError
      -1
    end
  end
end
