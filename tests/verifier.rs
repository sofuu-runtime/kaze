// v0.7: the bytecode verifier and the ISA/format locks.

use std::path::{Path, PathBuf};

use kaze::isa::{self, word, CapRef, Const, ItemCode, ItemTag, Module, Op};
use kaze::RunOptions;

fn item(code: Vec<u32>) -> ItemCode {
    ItemCode {
        name: "f".into(),
        tag: ItemTag::Action,
        params: Vec::new(),
        code,
        line: 1,
        origin: String::new(),
        period_ms: 0.0,
        slots: 0,
    }
}

fn module(code: Vec<u32>) -> Module {
    Module { items: vec![item(code)], ..Default::default() }
}

fn collect_kaze(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("examples") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            collect_kaze(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("kaze") {
            out.push(path);
        }
    }
}

#[test]
fn every_example_verifies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut files = Vec::new();
    collect_kaze(&root, &mut files);
    assert!(!files.is_empty());
    for path in files {
        let module = kaze::compile_file(&path.to_string_lossy())
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e.render()));
        isa::verify(&module)
            .unwrap_or_else(|why| panic!("{}: {}", path.display(), why));
    }
}

#[test]
fn a_jump_into_an_immediate_is_refused() {
    // JMP at 0, its immediate at 1, RET_NOTHING at 2. A -1 offset lands on the
    // immediate word.
    let m = module(vec![
        word(Op::Jmp, 0, 0, 0),
        (-1i32) as u32,
        word(Op::RetUnit, 0, 0, 0),
    ]);
    let err = isa::verify(&m).expect_err("a jump into an operand is refused");
    assert!(err.contains("instruction boundary"), "{}", err);
}

#[test]
fn a_jump_to_the_end_is_allowed() {
    let m = module(vec![word(Op::Jmp, 0, 0, 0), 0]);
    isa::verify(&m).expect("jumping past the last instruction is a clean halt");
}

#[test]
fn an_out_of_range_constant_is_refused() {
    let m = module(vec![word(Op::Const, 0, 0, 0), 99, word(Op::Halt, 0, 0, 0)]);
    let err = isa::verify(&m).expect_err("constant 99 does not exist");
    assert!(err.contains("constant 99"), "{}", err);
}

#[test]
fn an_out_of_range_capability_is_refused() {
    let m = module(vec![
        word(Op::CallCap, 0, 0, 0),
        0,
        word(Op::RetUnit, 0, 0, 0),
    ]);
    let err = isa::verify(&m).expect_err("capability 0 does not exist");
    assert!(err.contains("capability 0"), "{}", err);
}

#[test]
fn an_unregistered_capability_is_refused() {
    let mut m = module(vec![
        word(Op::CallCap, 0, 0, 0),
        0,
        word(Op::RetUnit, 0, 0, 0),
    ]);
    m.caps.push(CapRef { family: "drone".into(), op: "teleport".into(), argc: 0 });
    let err = isa::verify(&m).expect_err("drone.teleport is not in the registry");
    assert!(err.contains("not a registered capability"), "{}", err);
}

#[test]
fn a_stack_underflow_is_refused() {
    let m = module(vec![word(Op::Pop, 0, 0, 0), word(Op::RetUnit, 0, 0, 0)]);
    let err = isa::verify(&m).expect_err("POP on an empty stack is refused");
    assert!(err.contains("stack underflow"), "{}", err);
}

#[test]
fn branches_must_merge_at_the_same_stack_depth() {
    let mut m = module(vec![
        word(Op::Const, 0, 0, 0),
        0,
        word(Op::JmpIfFalse, 0, 0, 0),
        4, // target = 4 + 4 = 8
        word(Op::Const, 0, 0, 0),
        1,
        word(Op::Jmp, 0, 0, 0),
        0, // target = 8 + 0 = 8
        word(Op::RetUnit, 0, 0, 0),
    ]);
    m.consts.push(Const::Num(0.0));
    m.consts.push(Const::Num(1.0));
    let err = isa::verify(&m).expect_err("the true path leaves one value on the stack");
    assert!(err.contains("inconsistent stack depth"), "{}", err);
}

#[test]
fn a_truncated_immediate_is_refused() {
    let m = module(vec![word(Op::Const, 0, 0, 0)]);
    let err = isa::verify(&m).expect_err("CONST without its immediate is refused");
    assert!(err.contains("has no immediate"), "{}", err);
}

#[test]
fn an_unknown_opcode_is_refused() {
    let m = module(vec![0xff]);
    let err = isa::verify(&m).expect_err("opcode 255 is not defined");
    assert!(err.contains("unknown opcode"), "{}", err);
}

#[test]
fn deserialize_and_run_refuse_an_invalid_module() {
    let m = module(vec![word(Op::Const, 0, 0, 0), 99, word(Op::Halt, 0, 0, 0)]);
    let bytes = isa::serialize(&m);
    let err = isa::deserialize(&bytes).expect_err("deserialize verifies");
    assert!(err.contains("constant 99"), "{}", err);

    let err = kaze::run_module(m, "<bad>", &RunOptions::default())
        .expect_err("run_module verifies a hand-built module");
    assert_eq!(err.code, "E_KAZE_BADCODE");
}

#[test]
fn try_begin_at_nonzero_stack_depth_is_refused() {
    // CONST pushes one value; TRY_BEGIN at depth 1 cannot be restored by the
    // VM's truncate-based unwind.
    let mut m = module(vec![
        word(Op::Const, 0, 0, 0),
        0,
        word(Op::TryBegin, 255, 0, 0),
        1, // next 4, handler 5
        word(Op::RetUnit, 0, 0, 0),
        word(Op::Pop, 0, 0, 0),
        word(Op::RetUnit, 0, 0, 0),
    ]);
    m.consts.push(Const::Num(0.0));
    let err = isa::verify(&m).expect_err("TRY_BEGIN must start at depth 0");
    assert!(err.contains("TRY_BEGIN at stack depth"), "{}", err);
}

#[test]
fn a_capability_arity_outside_the_registry_is_refused() {
    let mut m = module(vec![
        word(Op::CallCap, 0, 0, 0),
        0,
        word(Op::RetUnit, 0, 0, 0),
    ]);
    // drone.goto is registered for 1..=3 values; zero is forged.
    m.caps.push(CapRef { family: "drone".into(), op: "goto".into(), argc: 0 });
    let err = isa::verify(&m).expect_err("the registry owns the arity");
    assert!(err.contains("takes 1 to 3"), "{}", err);
}

#[test]
fn a_try_module_verifies_including_its_handler_edge() {
    let src = "needs: log\naction main():\n    try:\n        log \"a\"\n    otherwise:\n        log \"b\"\n";
    let module = kaze::compile_source(src, "<try>").expect("compiles");
    isa::verify(&module).expect("the recovery block has a consistent stack");
}

#[test]
fn the_module_format_is_locked() {
    assert_eq!(isa::FORMAT_VERSION, 4);
}

#[test]
fn opcode_numbers_are_locked() {
    const LOCK: &[(&str, u8)] = &[
        ("CONST", 0),
        ("LOAD", 1),
        ("STORE", 2),
        ("NEWARR", 3),
        ("GETINDEX", 4),
        ("LEN", 5),
        ("POP", 6),
        ("ADD", 7),
        ("SUB", 8),
        ("MUL", 9),
        ("DIV", 10),
        ("MOD", 11),
        ("NEG", 12),
        ("CMP_LT", 13),
        ("CMP_LE", 14),
        ("CMP_GT", 15),
        ("CMP_GE", 16),
        ("CMP_EQ", 17),
        ("CMP_NE", 18),
        ("NOT", 19),
        ("JMP", 20),
        ("JMP_IF_FALSE", 21),
        ("CALL_CAP", 22),
        ("POLICY_STOP", 23),
        ("RET", 24),
        ("RET_NOTHING", 25),
        ("FUEL_TICK", 26),
        ("TRAP_ERR", 27),
        ("HALT", 28),
        ("JMP_IF_TRUE", 29),
        ("GET_FIELD", 30),
        ("JEQ", 31),
        ("JNE", 32),
        ("VECNEW", 33),
        ("VECDIST", 34),
        ("VECBEARING", 35),
        ("VECBEARING_BETWEEN", 36),
        ("SIN", 37),
        ("COS", 38),
        ("ATAN2", 39),
        ("SQRT", 40),
        ("DEG_TO_RAD", 41),
        ("RAD_TO_DEG", 42),
        ("VECSLANT_DIST", 43),
        ("TRY_BEGIN", 44),
        ("TRY_END", 45),
        ("LINK_LOST", 46),
    ];
    for (name, number) in LOCK {
        let op = Op::from_u8(*number).unwrap_or_else(|| panic!("opcode {} is missing", number));
        assert_eq!(op.name(), *name, "opcode {} was renamed", number);
        assert_eq!(op as u8, *number);
    }
    assert!(Op::from_u8(47).is_none(), "a new opcode needs a format bump");
}
