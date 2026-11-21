#!/usr/bin/env python3
"""Prueba shutdown y cancelación del hijo con límites externos y datos desechables."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import hashlib
import os
from pathlib import Path
import queue
import secrets
import signal
import socket
import sqlite3
import tempfile
import struct
import subprocess
import threading
import time

from verify import request

HERE = Path(__file__).resolve().parent


def receive(pipe):
    def exact(n):
        result = bytearray()
        while len(result) < n:
            part = pipe.read(n - len(result))
            if not part:
                raise EOFError('El proceso cerró stdout')
            result.extend(part)
        return result
    size = struct.unpack('>I', exact(4))[0]
    if not 0 < size <= 16 * 1024 * 1024:
        raise ValueError('Frame fuera de límite')
    return json.loads(exact(size))


class Child:
    def __init__(self, binary, source, log):
        self.log = log.open('wb')
        self.process = subprocess.Popen([binary, '--function-worker'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.log, env={}, cwd=HERE, start_new_session=True)
        self.messages = queue.Queue(maxsize=8)
        def read():
            try:
                while True:
                    self.messages.put(receive(self.process.stdout), timeout=1)
            except Exception as exc:
                try:
                    self.messages.put(exc, timeout=1)
                except queue.Full:
                    pass
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.generation = secrets.token_hex(32)
        self.sequence = 0
        self.send(dict(kind='Init', abi='konae-functions/1', function='orders', source=source,
            generation=self.generation,
            limits=dict(heap_bytes=16*1024*1024, response_bytes=65536, rss_bytes=256*1024*1024, operation_limit=8)))
        ready = self.recv()
        if ready != dict(kind='Ready', abi='konae-functions/1', generation=self.generation):
            raise AssertionError(ready)

    def send(self, value):
        data = json.dumps(value).encode()
        self.process.stdin.write(struct.pack('>I', len(data)) + data)
        self.process.stdin.flush()

    def recv(self, timeout=2):
        message = self.messages.get(timeout=timeout)
        if isinstance(message, Exception):
            raise message
        return message

    def invoke(self, actor="alice"):
        self.sequence += 1
        cap = dict(generation=self.generation, request=self.sequence, token=secrets.token_hex(32))
        self.send(dict(kind='Invoke', capability=cap,
            input=dict(url='http://localhost/a/orders/1', method='GET', headers=[], body=[], actor=actor)))
        return cap

    def close(self):
        try:
            os.killpg(self.process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        self.process.wait(timeout=2)
        for pipe in (self.process.stdin, self.process.stdout):
            pipe.close()
        self.reader.join(timeout=2)
        self.log.close()


@contextmanager
def child(binary, source, log):
    instance = Child.__new__(Child)
    try:
        instance.__init__(binary, source, log)
        yield instance
    finally:
        if hasattr(instance, 'process'):
            instance.close()


def child_pids(pid):
    rows = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True).splitlines()
    return [int(parts[0]) for row in rows if len(parts := row.split()) == 2 and int(parts[1]) == pid]


def exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


@contextmanager
def host(binary, source, log, output, extra=None):
    with tempfile.TemporaryDirectory(dir=output, prefix='host-') as temp:
        directory = Path(temp)
        (directory/'orders.js').write_text(source)
        (directory/'preferences.js').write_text(source)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        env = dict(os.environ, MODE='deno', PORT=str(port), DATA_DIR=str(directory/'data'),
                   APP_BUNDLE_DIR=str(directory), REQUEST_TIMEOUT_MS='1000',
                   WORKER_HEAP_BYTES=str(16*1024*1024), WORKER_RSS_BYTES=str(128*1024*1024),
                   WORKER_RESPONSE_BYTES='65536', WORKER_OPERATION_LIMIT='2')
        env.update(extra or {})
        with log.open('wb') as stream:
            process = subprocess.Popen([binary], env=env, stdout=stream, stderr=stream, start_new_session=True)
            process.data_dir = directory/'data'
            known = []
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    try:
                        if request(base, 'GET', '/health', timeout=.2)['status'] == 200:
                            break
                    except (OSError, TimeoutError):
                        pass
                    if process.poll() is not None:
                        raise RuntimeError('El host terminó antes de /health')
                    time.sleep(.02)
                else:
                    raise TimeoutError('/health no respondió')
                yield process, base, known
            finally:
                known.extend(child_pids(process.pid))
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=2)
                deadline = time.monotonic() + 2
                while any(exists(pid) for pid in known) and time.monotonic() < deadline:
                    time.sleep(.02)
                for pid in set(known):
                    if exists(pid):
                        os.killpg(pid, signal.SIGKILL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='/tmp/konae-deno-core-target/release/konae-functions-spike')
    parser.add_argument('--output-dir')
    args = parser.parse_args()
    output = Path(args.output_dir).resolve() if args.output_dir else HERE / ('hardening-worker-results-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S'))
    if not output.is_relative_to(HERE):
        parser.error('Los resultados deben quedar dentro de checks/')
    output.mkdir(parents=True, exist_ok=False)
    results = []
    def record(name, fn, source):
        started = time.monotonic()
        try:
            with child(args.binary, source, output/f'{name}.log') as instance:
                detail = fn(instance)
            results.append(dict(criterion=name, status='pass', detail=detail))
        except Exception as exc:
            results.append(dict(criterion=name, status='fail', detail=dict(error=f'{type(exc).__name__}: {exc}')))
        results[-1]['elapsed_ms'] = round((time.monotonic()-started)*1000, 2)
        print(name, results[-1]['status'], flush=True)

    def signal_case(instance):
        cap = instance.invoke()
        time.sleep(.03)
        instance.send(dict(kind='Cancel', capability=cap))
        result = instance.recv()
        assert result['kind'] == 'Reply' and json.loads(result['output']['body']) == {'aborted': True}, result
        return result
    record('request-signal-aborts', signal_case, '''
export function handle(request) {
  return new Promise(resolve => request.signal.addEventListener('abort',
    () => resolve(Response.json({aborted:request.signal.aborted})), {once:true}));
}''')

    def shutdown_case(instance):
        instance.send(dict(kind='Shutdown', grace_ms=100))
        result = instance.recv()
        assert result == dict(kind='ShutdownComplete', invoked=True, error=None), result
        return result
    record('shutdown-hook-runs', shutdown_case, '''
export function handle() { return Response.json({ok:true}); }
export function shutdown(signal) { if (signal.aborted) throw Error('early_abort'); }
''')
    record('shutdown-signal-aborts', shutdown_case, '''
export function handle() { return Response.json({ok:true}); }
export function shutdown(signal) {
  return new Promise(resolve => signal.addEventListener('abort', resolve, {once:true}));
}
''')

    def expired(instance):
        instance.invoke()
        first = instance.recv()
        assert first['kind'] == 'Reply', first
        instance.send(dict(kind='Shutdown', grace_ms=100))
        result = instance.recv()
        assert result == dict(kind='ShutdownComplete', invoked=True, error='capability_expired'), result
        return result
    record('shutdown-has-no-capability', expired, '''
let saved;
export function handle(request, context) { saved=context.data.readOrder; return Response.json({ok:true}); }
export function shutdown() { try { saved(1); } catch {} }
''')

    def eof(instance):
        instance.invoke()
        time.sleep(.03)
        instance.process.stdin.close()
        code = instance.process.wait(timeout=2)
        assert code != 0, code
        return dict(exit_code=code, reason='EOF del supervisor interrumpe el proceso aunque JS no coopere')
    record('parent-pipe-eof-during-loop', eof, 'export function handle(){ while(true){} }')

    def invalid_utf8(instance):
        instance.invoke()
        result = instance.recv()
        assert result['kind'] == 'Failure' and result['code'] == 'invalid_response', result
        return result
    record('invalid-utf8-refused', invalid_utf8, 'export function handle(){return new Response(new Uint8Array([255]));}')

    def system_timer(instance):
        instance.invoke()
        result = instance.recv()
        assert result['kind'] == 'Failure' and result['code'] == 'unsupported_runtime_api', result
        return result
    record('system-timer-cannot-survive-request', system_timer, '''
export function handle(){
  try { AbortSignal.timeout(10000); } catch {}
  return Response.json({ok:true});
}''')


    def action_promise(instance):
        cap = instance.invoke()
        operation = instance.recv()
        assert operation['kind'] == 'Operation' and operation['operation'] == 'action', operation
        instance.send(dict(kind='OperationResult', capability=cap, sequence=1,
                           result={'Err':'forbidden'}))
        result = instance.recv()
        assert result['kind'] == 'Failure' and result['code'] == 'forbidden', result
        return result
    record('actions-promise-rejects-canonically', action_promise, """
export async function handle(request, context) {
  const value = context.actions.call('private', {});
  if (!(value instanceof Promise)) return Response.json({bad:'not_promise'});
  try { await value; } catch { return Response.json({caught:true}); }
  return Response.json({bad:'not_rejected'});
}
""")

    def anonymous(instance):
        instance.invoke(actor='')
        result = instance.recv()
        assert result['kind'] == 'Reply' and json.loads(result['output']['body']) == {'actor':None}, result
        return result
    record('anonymous-actor-is-null', anonymous,
           'export function handle(request,context){return Response.json({actor:context.actor.userId});}')

    def http_case(name, source, check, extra=None):
        started = time.monotonic()
        try:
            with host(args.binary, source, output/f'{name}.log', output, extra) as handle:
                detail = check(*handle)
            results.append(dict(criterion=name, status='pass', detail=detail))
        except Exception as exc:
            results.append(dict(criterion=name, status='fail', detail=dict(error=f'{type(exc).__name__}: {exc}')))
        results[-1]['elapsed_ms'] = round((time.monotonic()-started)*1000,2)
        print(name,results[-1]['status'],flush=True)

    def bounded(code):
        def check(process, base, known):
            response = request(base, 'GET', '/a/orders/1', headers={'Cookie':'session=a-alice'},timeout=3)
            assert response['status'] >= 400 and response['body'] == {'error':code}, response
            healthy = request(base,'GET','/health')
            assert healthy['status'] == 200, healthy
            deadline = time.monotonic()+2
            while child_pids(process.pid) and time.monotonic()<deadline:
                time.sleep(.02)
            assert not child_pids(process.pid), child_pids(process.pid)
            return dict(response=response, health=healthy['body'], worker_reaped=True)
        return check
    http_case('response-byte-limit', 'export function handle(){return new Response("é".repeat(32769));}',
              bounded('response_too_large'))
    http_case('operation-count-limit', """
export function handle(request,context) {
  try { for(let i=0;i<3;i++)context.data.readOrder(1); } catch {}
  return Response.json({ok:true});
}
""", bounded('operation_limit'))
    http_case('heap-failure-contained', """
export function handle(){
  globalThis.retained=Array.from({length:200000},()=>({text:'x'.repeat(512)}));
  return Response.json({count:retained.length});
}
""", bounded('worker_failed'))
    http_case('rss-poll-detects-finite-burst', """
export function handle(){
  globalThis.retained=new Uint8Array(112*1024*1024).fill(7);
  return new Promise(()=>{});
}
""", bounded('worker_memory_limit'))

    def shutdown_hung(process,base,known):
        response=request(base,'GET','/a/orders/1',headers={'Cookie':'session=a-alice'})
        assert response['status']==200,response
        children=child_pids(process.pid)
        known.extend(children)
        assert len(children)==1,children
        start=time.monotonic()
        evicted=request(base,'POST','/__control/evict',b'',timeout=2)
        elapsed=time.monotonic()-start
        assert evicted['status']==200 and elapsed<.75,(evicted,elapsed)
        assert not any(exists(pid) for pid in children),children
        assert request(base,'GET','/health')['status']==200
        return dict(eviction=evicted['body'],elapsed_ms=round(elapsed*1000,2),worker_reaped=True)
    http_case('hung-shutdown-killed-and-reaped',
              'export function handle(){return Response.json({ok:true});} export function shutdown(){while(true){}}',
              shutdown_hung)

    def killed_parent(process,base,known):
        observed={}
        def invoke():
            try:
                observed['response']=request(base,'GET','/a/orders/1',headers={'Cookie':'session=a-alice'},timeout=3)
            except Exception as exc:
                observed['error']=type(exc).__name__
        caller=threading.Thread(target=invoke,daemon=True)
        caller.start()
        deadline=time.monotonic()+2
        children=[]
        while time.monotonic()<deadline:
            children=child_pids(process.pid)
            if children:
                break
            time.sleep(.02)
        assert len(children)==1,children
        known.extend(children)
        time.sleep(.05)
        os.kill(process.pid,signal.SIGKILL)
        process.wait(timeout=2)
        deadline=time.monotonic()+2
        while any(exists(pid) for pid in children) and time.monotonic()<deadline:
            time.sleep(.02)
        assert not any(exists(pid) for pid in children),children
        caller.join(timeout=3)
        return dict(parent_exit=process.returncode,worker_disappeared=True,request=observed)
    http_case('parent-sigkill-leaves-no-worker', 'export function handle(){while(true){}}',
              killed_parent, {'REQUEST_TIMEOUT_MS':'10000'})

    def foreign_row(process,base,known):
        def rows():
            db=sqlite3.connect((process.data_dir/'a.sqlite').as_uri()+'?mode=ro',uri=True)
            try:
                return {name:db.execute('SELECT * FROM '+name).fetchall()
                        for name in ('orders','audit','sessions','mutation_receipts')}
            finally:
                db.close()
        before=rows()
        response=request(base,'POST','/a/orders/2',b'{"quantity":3}',
                         {'Cookie':'session=a-alice','Idempotency-Key':'foreign-row-test'})
        after=rows()
        assert response['status']==404 and response['body']=={'error':'not_found'},response
        assert before==after,(before,after)
        return dict(response=response,database_unchanged=True)
    http_case('foreign-order-leaves-no-receipt', '''
export function handle(request,context){
  const order=context.data.updateOrder(2,3);
  return order?Response.json(order):Response.json({error:'not_found'},{status:404});
}''',foreign_row)

    def preferences_cannot_mutate(process,base,known):
        headers={'Cookie':'session=a-alice'}
        responses={operation:request(base,'GET',f'/a/preferences?op={operation}',headers=headers)
                   for operation in ('read', 'update')}
        for response in responses.values():
            assert response['status']==500 and response['body']=={'error':'internal_error'},response
        db=sqlite3.connect((process.data_dir/'a.sqlite').as_uri()+'?mode=ro',uri=True)
        try:
            assert db.execute('SELECT quantity FROM orders WHERE id=1').fetchone()==(2,)
            assert db.execute('SELECT COUNT(*) FROM audit').fetchone()==(0,)
            assert db.execute('SELECT COUNT(*) FROM mutation_receipts').fetchone()==(0,)
        finally:
            db.close()
        return dict(responses=responses,database_unchanged=True)
    http_case('fixture-preferences-has-no-order-ops', '''
export function handle(request,context){
  try {
    if (new URL(request.url).searchParams.get('op') === 'read') context.data.readOrder(1);
    else context.data.updateOrder(1,8);
  } catch {}
  return Response.json({ok:true});
}''',preferences_cannot_mutate)

    document = dict(results=results, summary={s:sum(r['status']==s for r in results) for s in ('pass','fail')},
        binary_sha256=hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(),
        script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        scope='IPC y procesos propios. Las asignaciones son finitas; RSS polling no demuestra límite duro.')
    (output/'results.json').write_text(json.dumps(document,ensure_ascii=False,indent=2)+'\n')
    print(output)
    return int(document['summary']['fail'] > 0)


if __name__ == '__main__':
    raise SystemExit(main())
