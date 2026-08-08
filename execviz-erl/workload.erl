% =========================================================================
%  MANIFEST
% =========================================================================
%  script_name: workload.erl
%  script_path: execviz-erl/workload.erl
%  module_name: workload
%  version: 0.53.1
%  description: %% A BEAM service, traced: requests fan in across processes, a worker claims %% stamped work off a mailbox, one request fails, and a lock never releases.
%  kind: module
%  spec: internal
%  internal_dependencies: 
%  external_dependencies: 
%  features: workload
%  api_version: execvis-v1.0.0
%  last_updated: 2026-08-07
% =========================================================================

%%% A BEAM service, traced: requests fan in across processes, a worker claims
%%% stamped work off a mailbox, one request fails, and a lock never releases.
-module(workload).
%% ========================================================================
%% INTERFACE
%% ========================================================================
-export([main/0]).

%% ========================================================================
%% IMPLEMENTATION
%% ========================================================================
main() ->
    Collector = case os:getenv("EXECVIZ_COLLECTOR") of
        false -> "http://127.0.0.1:8900";
        C -> C
    end,
    execviz:install(Collector, <<"beam-1">>, <<"api">>),
    Root = execviz:span_start(service, call),
    put(execviz_span, Root),

    %% a lock that is never released: an unfinished span, which on the BEAM is
    %% the case that matters most since a process can vanish at any moment
    Stuck = execviz:span_start(reconcile_lock, wait, [{domain, <<"billing">>}]),
    execviz:lifecycle(Stuck, suspended),

    Jobs = lists:foldl(fun(Uid, Acc) ->
        execviz:with_span(io_lib:format("GET /profile/~p", [Uid]), call, fun() ->
            execviz:gather(profile_fanin, [
                fun() -> execviz:with_span(fetch_user, call, fun() ->
                            execviz:log(info, "loading user"),
                            execviz:with_span(db_user, io, fun() -> timer:sleep(40) end)
                         end) end,
                fun() -> execviz:with_span(fetch_orders, call, fun() ->
                            execviz:with_span(db_orders, io, fun() ->
                                timer:sleep(60),
                                case Uid of
                                    2 -> execviz:log(error, "order store unavailable"),
                                         erlang:error(order_store_unavailable);
                                    _ -> ok
                                end
                            end)
                         end) end
            ]),
            execviz:with_span(render, call, fun() -> timer:sleep(20) end),
            Q = execviz:span_start(enqueue_job, queue),
            Stamped = execviz:stamp(io_lib:format("invoice-~p", [Uid])),
            [{Stamped, Q} | Acc]
        end)
    end, [], [0, 1, 2]),

    %% a worker process: it inherits nothing implicitly, and claims what it is
    %% handed
    Self = self(),
    Worker = spawn(fun() ->
        execviz:install(Collector, <<"beam-1">>, <<"worker">>),
        lists:foreach(fun({Msg, QId}) ->
            {Item, _} = execviz:claim(Msg),
            execviz:with_span(io_lib:format("process_~s", [Item]), call, fun() ->
                execviz:log(info, io_lib:format("processing ~s", [Item])),
                timer:sleep(30)
            end),
            execviz:release(QId)
        end, Jobs),
        Self ! done
    end),
    receive done -> ok after 20000 -> timeout end,
    _ = Worker,

    execviz:span_end(Root, ok),
    execviz:flush(),
    io:format("beam workload complete~n"),
    halt(0).
