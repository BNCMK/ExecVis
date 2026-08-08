# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: workload.rb
#  script_path: execviz-ruby/workload.rb
#  module_name: workload
#  version: 0.53.1
#  description: A real Ruby service, traced. Requests run concurrently, a worker claims stamped work off a queue, one request fails, and a lock never releases.
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: workload
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# A real Ruby service, traced. Requests run concurrently, a worker claims stamped
# work off a queue, one request fails, and a lock never releases.
require_relative 'execviz'

Execviz.install(host_id: 'ruby-1', domain: 'api', flush_secs: 0.3)

jobs = Queue.new
root = Execviz.span_start('service', 'call')
Fiber[:execviz_span] = root

stuck = Execviz.span_start('reconcile_lock', 'wait', domain: 'billing')
Execviz.lifecycle(stuck, 'suspended')

worker = Execviz.thread('worker_loop') do
  Execviz.set_domain('worker')
  loop do
    msg = jobs.pop
    break if msg == :stop

    item, qid = Execviz.claim(msg)
    Execviz.span("process_#{item}", 'call') do
      Execviz.log(:info, "processing #{item}")
      sleep 0.03
    end
    Execviz.release(qid)
  end
end

failed = 0
3.times do |uid|
  Execviz.span("GET /profile/#{uid}", 'call', domain: 'api') do
    Execviz.gather('profile_fanin', [
                     -> {
                       Execviz.span("fetch_user_#{uid}", 'call', domain: 'users') do
                         Execviz.log(:info, "loading user #{uid}")
                         Execviz.span('db_user', 'io') { sleep 0.04 + uid * 0.015 }
                       end
                     },
                     -> {
                       Execviz.span("fetch_orders_#{uid}", 'call', domain: 'orders') do
                         Execviz.span('db_orders', 'io') do
                           sleep 0.06
                           if uid == 2
                             Execviz.log(:error, 'order store unavailable')
                             raise 'order store unavailable'
                           end
                         end
                       end
                     }
                   ])
    Execviz.span("render_#{uid}", 'call', domain: 'render') { sleep 0.02 }
    qid = Execviz.span_start('enqueue_job', 'queue')
    m = Execviz.stamp("invoice-#{uid}")
    m['span'] = qid
    jobs << m
  end
rescue StandardError
  failed += 1
end

sleep 0.4
jobs << :stop
worker.join
Execviz.span_end(root, 'ok')
Execviz.shutdown
warn "ruby workload complete, #{failed} failed"
