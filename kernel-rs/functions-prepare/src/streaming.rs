use anyhow::{Context, Result, ensure};
use wirm::wasmparser::MemArg;
use wirm::{
    DataType, InitInstr, Module, Opcode,
    ir::{
        function::FunctionBuilder,
        id::{FunctionID, GlobalID, LocalID},
        types::{BlockType, InitExpr, Value},
    },
    module_builder::AddLocal,
    wasmparser::Operator,
};

const CHUNK_BYTES: i32 = 64 * 1024;
const STACK: GlobalID = GlobalID(0);

pub fn forward_random(core: &[u8]) -> Result<Vec<u8>> {
    let mut module = Module::parse(core, false, false)?;
    let random = module
        .imports
        .get_func(
            "wasi:random/random@0.2.10".into(),
            "get-random-bytes".into(),
        )
        .context("StarlingMonkey random import changed")?;
    let realloc = module
        .exports
        .get_func_by_name("cabi_realloc".into())
        .context("StarlingMonkey allocator changed")?;
    let random_get = module
        .functions
        .get_local_fid_by_name("random_get")
        .context("StarlingMonkey random_get stub changed")?;
    let mut function = FunctionBuilder::new(&[DataType::I32, DataType::I32], &[DataType::I32]);
    for ty in [DataType::I32; 3] {
        function.add_local(ty);
    }
    function
        .i32_const(0)
        .i32_const(0)
        .i32_const(4)
        .i32_const(8)
        .call(realloc)
        .local_set(LocalID(2))
        .local_get(LocalID(1))
        .i64_extend_i32_u()
        .local_get(LocalID(2))
        .call(random)
        .local_get(LocalID(2));
    function.body.push_op(Operator::I32Load {
        memarg: MemArg {
            align: 2,
            max_align: 2,
            offset: 0,
            memory: 0,
        },
    });
    function.local_set(LocalID(3)).local_get(LocalID(2));
    function.body.push_op(Operator::I32Load {
        memarg: MemArg {
            align: 2,
            max_align: 2,
            offset: 4,
            memory: 0,
        },
    });
    function
        .local_set(LocalID(4))
        .local_get(LocalID(0))
        .local_get(LocalID(3))
        .local_get(LocalID(4));
    function.body.push_op(Operator::MemoryCopy {
        dst_mem: 0,
        src_mem: 0,
    });
    function
        .local_get(LocalID(3))
        .local_get(LocalID(4))
        .i32_const(1)
        .i32_const(0)
        .call(realloc)
        .drop()
        .local_get(LocalID(2))
        .i32_const(8)
        .i32_const(4)
        .i32_const(0)
        .call(realloc)
        .drop()
        .i32_const(0);
    function.end();
    module
        .functions
        .get_mut(random_get)
        .unwrap_local_mut()?
        .body = function.body;
    Ok(module.encode()?)
}

pub fn bound_allocations(core: &[u8]) -> Result<Vec<u8>> {
    let mut module = Module::parse(core, false, false)?;
    let (read, read_call) = trampoline(&module, "op-worker-body-read")?;
    let (write, _) = trampoline(&module, "op-worker-response-chunk")?;
    let realloc = module
        .exports
        .get_func_by_name("cabi_realloc".into())
        .context("StarlingMonkey allocator changed")?;
    let in_read = i32_global(&mut module);
    let buffer = i32_global(&mut module);
    let record = i32_global(&mut module);
    let checkpoints = i32_global(&mut module);
    let collect = collector(&module)?;
    let checkpoint = gc_checkpoint(checkpoints, collect).finish_module(&mut module);

    // The canonical ABI copies each host chunk into this buffer. The JS
    // bindings take their own copy before the next read can reuse it.
    let mut allocate = FunctionBuilder::new(&[DataType::I32; 4], &[DataType::I32]);
    allocate.global_get(in_read).if_stmt(BlockType::Empty);
    allocate
        .local_get(LocalID(0))
        .local_get(LocalID(1))
        .i32_or()
        .local_get(LocalID(2))
        .i32_const(1)
        .i32_ne()
        .i32_or()
        .local_get(LocalID(3))
        .i32_const(CHUNK_BYTES)
        .i32_gt_u()
        .i32_or()
        .if_stmt(BlockType::Empty)
        .unreachable()
        .end();
    allocate_once(&mut allocate, buffer, realloc, 1, CHUNK_BYTES);
    allocate.global_get(buffer).return_stmt().end();
    for parameter in 0..4 {
        allocate.local_get(LocalID(parameter));
    }
    allocate.call(realloc);
    let allocate = allocate.finish_module(&mut module);
    module
        .exports
        .add_export_func("cabi_import_realloc".into(), *allocate);

    let read_function = module.functions.get_mut(read).unwrap_local_mut()?;
    let instructions = read_function.body.instructions.get_ops_mut()?;
    let allocation = [
        Operator::I32Const { value: 0 },
        Operator::I32Const { value: 0 },
        Operator::I32Const { value: 4 },
        Operator::I32Const { value: 8 },
        Operator::Call {
            function_index: *realloc,
        },
    ];
    let start = instructions
        .windows(allocation.len())
        .position(|window| window == allocation)
        .context("StarlingMonkey body record allocation changed")?;
    let mut reuse_record = FunctionBuilder::new(&[], &[]);
    allocate_once(&mut reuse_record, record, realloc, 4, 8);
    reuse_record.global_get(record);
    let after = [
        Operator::I32Const { value: 0 },
        Operator::GlobalSet {
            global_index: *in_read,
        },
    ];
    let before = [
        Operator::I32Const { value: 1 },
        Operator::GlobalSet {
            global_index: *in_read,
        },
    ];
    ensure!(
        read_call >= start + allocation.len(),
        "StarlingMonkey body trampoline changed"
    );
    instructions.splice(read_call + 1..read_call + 1, after);
    instructions.splice(read_call..read_call, before);
    instructions.splice(
        start..start + allocation.len(),
        reuse_record.body.instructions.get_ops().iter().cloned(),
    );
    instructions.insert(
        0,
        Operator::Call {
            function_index: *checkpoint,
        },
    );
    read_function.body.num_instructions = instructions.len();

    let write_function = module.functions.get_mut(write).unwrap_local_mut()?;
    let instructions = write_function.body.instructions.get_ops_mut()?;
    instructions.insert(
        0,
        Operator::Call {
            function_index: *checkpoint,
        },
    );
    write_function.body.num_instructions = instructions.len();

    Ok(module.encode()?)
}

fn i32_global(module: &mut Module<'_>) -> GlobalID {
    module.add_global(
        InitExpr::new(vec![InitInstr::Value(Value::I32(0))]),
        DataType::I32,
        true,
        false,
    )
}

fn allocate_once(
    function: &mut FunctionBuilder<'_>,
    pointer: GlobalID,
    realloc: FunctionID,
    alignment: i32,
    bytes: i32,
) {
    function
        .global_get(pointer)
        .i32_eqz()
        .if_stmt(BlockType::Empty)
        .i32_const(0)
        .i32_const(0)
        .i32_const(alignment)
        .i32_const(bytes)
        .call(realloc)
        .global_set(pointer)
        .end();
}

fn trampoline(module: &Module<'_>, name: &str) -> Result<(FunctionID, usize)> {
    let import = module
        .imports
        .get_func("konae:worker/host".into(), name.into())
        .with_context(|| format!("Missing StarlingMonkey import {name}"))?;
    let call = Operator::Call {
        function_index: *import,
    };
    let mut callers = module
        .functions
        .iter()
        .filter_map(|function| function.unwrap_local().ok())
        .filter_map(|function| {
            function
                .body
                .instructions
                .get_ops()
                .iter()
                .position(|op| *op == call)
                .map(|position| (function.func_id, position))
        });
    let caller = callers
        .next()
        .context("Missing StarlingMonkey trampoline")?;
    ensure!(
        callers.next().is_none(),
        "Ambiguous StarlingMonkey trampoline"
    );
    Ok(caller)
}

fn collector<'a>(module: &Module<'a>) -> Result<Vec<Operator<'a>>> {
    // This sequence invokes SpiderMonkey's full GC. build.rs pins the engine
    // digest. The offsets identify the sequence in that engine.
    for function in module
        .functions
        .iter()
        .filter_map(|func| func.unwrap_local().ok())
    {
        let ops = function.body.instructions.get_ops();
        let Some(marker) = ops
            .iter()
            .position(|op| matches!(op, Operator::I32Store { memarg } if memarg.offset == 4632))
        else {
            continue;
        };
        let end = (marker.saturating_sub(34)..marker)
            .rev()
            .find(|&i| matches!(ops[i], Operator::Call { .. }))
            .context("StarlingMonkey GC call changed")?;
        let start = (end.saturating_sub(180)..end)
            .rev()
            .find(|&i| {
                matches!(ops[i], Operator::I32Const { value: 2391180 })
                    && ops[i..(i + 9).min(ops.len())].iter().any(
                        |op| matches!(op, Operator::I32Store8 { memarg } if memarg.offset == 3428),
                    )
            })
            .context("StarlingMonkey GC setup changed")?;
        return Ok(ops[start..=end].to_vec());
    }
    anyhow::bail!("Missing StarlingMonkey GC checkpoint")
}

fn gc_checkpoint<'a>(count: GlobalID, collect: Vec<Operator<'a>>) -> FunctionBuilder<'a> {
    let mut checkpoint = FunctionBuilder::new(&[], &[]);
    for _ in 0..7 {
        checkpoint.add_local(DataType::I32);
    }
    checkpoint
        .global_get(count)
        .i32_const(1)
        .i32_add()
        .local_tee(LocalID(0))
        .global_set(count)
        .local_get(LocalID(0))
        .i32_const(31)
        .i32_and()
        .if_stmt(BlockType::Empty)
        .return_stmt()
        .end()
        .global_get(STACK)
        .i32_const(112)
        .i32_sub()
        .local_tee(LocalID(5))
        .global_set(STACK);
    for op in collect {
        checkpoint.body.push_op(op);
    }
    checkpoint
        .local_get(LocalID(5))
        .i32_const(112)
        .i32_add()
        .global_set(STACK);
    checkpoint
}
