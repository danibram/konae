import sys

source, destination = sys.argv[1:]
with open(source) as stream:
    lines = stream.readlines()

# The pinned ComponentizeJS binary names this canonical body import once.
scratch = '''
  (core module $konae_body_scratch
    (import "guest" "realloc" (func $alloc (param i32 i32 i32 i32) (result i32)))
    (global $ptr (mut i32) (i32.const 0))
    (func (export "realloc") (param $old i32) (param $old-size i32)
          (param $align i32) (param $size i32) (result i32)
      local.get $old
      local.get $old-size
      i32.or
      local.get $align
      i32.const 1
      i32.ne
      i32.or
      local.get $size
      i32.const 65536
      i32.gt_u
      i32.or
      if
        unreachable
      end
      global.get $ptr
      i32.eqz
      if
        i32.const 0
        i32.const 0
        i32.const 1
        i32.const 65536
        call $alloc
        global.set $ptr
      end
      global.get $ptr
    )
  )
  (core instance $konae_body_scratch_imports (export "realloc" (func $cabi_realloc)))
  (core instance $konae_body_scratch_instance (instantiate $konae_body_scratch
    (with "guest" (instance $konae_body_scratch_imports))))
  (alias core export $konae_body_scratch_instance "realloc" (core func $konae_body_realloc))
'''

body_lowerings = []
for index, line in enumerate(lines):
    if '(canon lower (func $op-worker-body-read)' in line:
        if '(realloc $cabi_realloc)' in line:
            lines[index] = line.replace('(realloc $cabi_realloc)', '(realloc $konae_body_realloc)')
        elif '(realloc $konae_body_realloc)' not in line:
            print(f'Unexpected body-read lowering: {line.strip()}', file=sys.stderr)
            raise SystemExit('Body-read lowering has an unexpected allocator')
        body_lowerings.append(index)
if not body_lowerings:
    raise SystemExit('No body-read lowering found')
if '(core module $konae_body_scratch' not in ''.join(lines):
    lines.insert(body_lowerings[0], scratch)

# This sequence calls SpiderMonkey's private full-GC entry point. The builder
# checks the pinned engine digest before invoking this probe.
start = next(i for i, line in enumerate(lines) if 'i32.store offset=4632' in line)
end = next(i for i in range(start - 1, start - 35, -1) if 'call 10820' in lines[i])
begin = next(
    i for i in range(end - 180, end)
    if 'i32.const 2391180' in lines[i]
    and 'i32.store8 offset=3428' in ''.join(lines[i:i + 9])
)
gc_body = ''.join(lines[begin:end + 1])
def trampoline_for_call(target):
    calls = [i for i, line in enumerate(lines) if line.strip() == f'call {target}']
    if len(calls) != 1:
        raise SystemExit(f'Expected one trampoline call to host function {target}')
    return max(i for i in range(calls[0]) if lines[i].startswith('    (func (;'))

body_read_trampoline = trampoline_for_call(1)
response_chunk_trampoline = trampoline_for_call(2)
return_record = '''
      local.get 2
      global.get $konae_body_return_record
      i32.eqz
      if
        i32.const 0
        i32.const 0
        i32.const 4
        i32.const 8
        call 4443
        global.set $konae_body_return_record
      end
      global.get $konae_body_return_record
      local.tee 3
'''
allocation = '''      local.get 2
      i32.const 0
      i32.const 0
      i32.const 4
      i32.const 8
      call 4443
      local.tee 3
'''
# Insert from the later function first so the body-read index remains valid.
if 'call $konae_body_gc_checkpoint' in ''.join(lines[response_chunk_trampoline:response_chunk_trampoline + 20]):
    raise SystemExit('Response chunk checkpoint already present')
lines.insert(response_chunk_trampoline + 2, '      call $konae_body_gc_checkpoint\n')
trampoline_text = ''.join(lines[body_read_trampoline:body_read_trampoline + 30])
if allocation not in trampoline_text:
    raise SystemExit('Pinned body-read return record allocation changed')
lines[body_read_trampoline:body_read_trampoline + 30] = trampoline_text.replace(
    allocation, return_record, 1
).splitlines(keepends=True)
lines.insert(body_read_trampoline + 2, '      call $konae_body_gc_checkpoint\n')
insertion = next(i for i, line in enumerate(lines) if line.startswith('    (data '))
lines.insert(insertion, '''
    (global $konae_body_return_record (mut i32) (i32.const 0))
    (global $konae_body_reads (mut i32) (i32.const 0))
    (func $konae_body_gc_checkpoint
      (local i32 i32 i32 i32 i32 i32 i32)
      global.get $konae_body_reads
      i32.const 1
      i32.add
      local.tee 0
      global.set $konae_body_reads
      local.get 0
      i32.const 31
      i32.and
      if
        return
      end
      global.get 0
      i32.const 112
      i32.sub
      local.tee 5
      global.set 0
''' + gc_body + '''
      local.get 5
      i32.const 112
      i32.add
      global.set 0
    )
''')

with open(destination, 'w') as stream:
    stream.writelines(lines)
